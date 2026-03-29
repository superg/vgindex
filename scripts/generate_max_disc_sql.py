#!/usr/bin/env python3
"""Scan all scraped Redump JSON files, compute global maxima for every field
(max count for arrays, longest value for scalars), then emit a single SQL file
that replaces disc id=1 with a synthetic max-complexity entry.

Usage:
    python scripts/generate_max_disc_sql.py \
        --data-dir data/redump/db \
        --output max_disc_id1.sql
"""

import argparse
import json
import os
import sys
from datetime import datetime

from generate_import_sql import (
    LANG3_TO_LANG2,
    MEDIA_NAME_TO_CODE,
    REGION_NAME_TO_CODE,
    build_protection,
    build_keys,
    build_submission_data,
    merge_dumpers,
    merge_editions,
    merge_offsets,
    parse_hex_bca,
    parse_hex_dump_with_address,
    parse_hex_raw_spaced,
    parse_ring_entry,
    parse_ss_ranges,
    parse_track_hashes,
    split_csv,
    sql_bool,
    sql_bytea,
    sql_int,
    sql_int4range_array,
    sql_int_array,
    sql_jsonb,
    sql_str,
    sql_str_or_null,
    sql_text_array,
    sql_timestamp,
)


def _keep_longest(current, candidate):
    """Return the longer of two strings (by len), preferring non-None."""
    if candidate is None:
        return current
    if current is None:
        return candidate
    return candidate if len(str(candidate)) > len(str(current)) else current


def _keep_longest_list(current_list, candidate_list):
    """Return the list with more elements."""
    if not candidate_list:
        return current_list
    if not current_list:
        return list(candidate_list)
    return list(candidate_list) if len(candidate_list) > len(current_list) else current_list


class MaxStats:
    """Accumulates global max counts and longest values across all scrape files."""

    def __init__(self):
        self.longest_title = None
        self.longest_title_foreign = None
        self.longest_disc_title = None
        self.longest_disc_number = None
        self.longest_version = None
        self.longest_filename_suffix = None
        self.longest_comments = None
        self.longest_contents = None
        self.longest_exe_date = None

        self.longest_serial_element = None
        self.max_serial_count = 0
        self.serial_elements = []

        self.longest_edition_element = None
        self.max_edition_count = 0
        self.edition_elements = []

        self.longest_barcode_element = None
        self.max_barcode_count = 0
        self.barcode_elements = []

        self.max_error_count = None

        self.max_layerbreak_count = 0
        self.layerbreak_values = []

        self.longest_pvd_hex = None
        self.longest_pic_hex = None
        self.longest_bca_hex = None
        self.longest_header_hex = None

        self.longest_protection = None
        self.longest_sbi = None

        self.max_sector_ranges_count = 0
        self.sector_ranges_values = []
        self.longest_protection_range_repr = None

        self.max_protection_keys_count = 0
        self.longest_protection_key = None
        self.protection_key_elements = []

        self.longest_cue = None

        self.max_region_count = 0
        self.region_codes = []

        self.max_language_count = 0
        self.language_codes = []

        self.max_ring_entry_count = 0
        self.max_ring_layer_count = 0
        self.longest_mastering_code = None
        self.longest_mastering_sid = None
        self.max_mould_sids_count = 0
        self.longest_mould_sid = None
        self.mould_sid_elements = []
        self.max_toolstamps_count = 0
        self.longest_toolstamp = None
        self.toolstamp_elements = []
        self.max_additional_moulds_count = 0
        self.longest_additional_mould = None
        self.additional_mould_elements = []
        self.max_offset_values_count = 0

        self._ring_pool_size = 100
        self.mastering_code_pool = []
        self.mastering_sid_pool = []
        self.mould_sid_pool = []
        self.toolstamp_pool = []
        self.additional_mould_pool = []

        self.max_track_count = 0
        self.longest_track_size = 0

        self.max_dumper_count = 0
        self.dumper_names = []

        self.max_changes_count = 0
        self.longest_change_payload = None
        self.max_change_fields_count = 0
        self.longest_change_field_entry = None
        self.change_payloads = []

        self.files_processed = 0
        self.files_skipped = 0

    def _add_to_pool(self, pool, value):
        """Add a non-empty value to a pool, keeping the top N by length, all unique."""
        if not value or not str(value).strip():
            return
        val = str(value).strip()
        if val in pool:
            return
        if len(pool) < self._ring_pool_size:
            pool.append(val)
        else:
            shortest_idx = min(range(len(pool)), key=lambda i: len(pool[i]))
            if len(val) > len(pool[shortest_idx]):
                pool[shortest_idx] = val

    def ingest(self, data):
        self.files_processed += 1

        self.longest_title = _keep_longest(self.longest_title, data.get("d_title"))
        self.longest_title_foreign = _keep_longest(self.longest_title_foreign, data.get("d_title_foreign"))
        self.longest_disc_title = _keep_longest(self.longest_disc_title, data.get("d_label"))
        self.longest_disc_number = _keep_longest(self.longest_disc_number, data.get("d_number"))
        self.longest_version = _keep_longest(self.longest_version, data.get("d_version"))
        self.longest_filename_suffix = _keep_longest(self.longest_filename_suffix, data.get("d_version_datfile"))
        self.longest_comments = _keep_longest(self.longest_comments, data.get("d_comments"))
        self.longest_contents = _keep_longest(self.longest_contents, data.get("d_contents"))
        self.longest_exe_date = _keep_longest(self.longest_exe_date, data.get("d_date"))

        serials = split_csv(data.get("d_serial"))
        if len(serials) > self.max_serial_count:
            self.max_serial_count = len(serials)
            self.serial_elements = list(serials)
        for s in serials:
            self.longest_serial_element = _keep_longest(self.longest_serial_element, s)

        editions = merge_editions(data)
        if len(editions) > self.max_edition_count:
            self.max_edition_count = len(editions)
            self.edition_elements = list(editions)
        for e in editions:
            self.longest_edition_element = _keep_longest(self.longest_edition_element, e)

        barcodes = split_csv(data.get("d_barcode"))
        if len(barcodes) > self.max_barcode_count:
            self.max_barcode_count = len(barcodes)
            self.barcode_elements = list(barcodes)
        for b in barcodes:
            self.longest_barcode_element = _keep_longest(self.longest_barcode_element, b)

        if data.get("d_errors"):
            try:
                ec = int(data["d_errors"])
                if self.max_error_count is None or ec > self.max_error_count:
                    self.max_error_count = ec
            except ValueError:
                pass

        lb = data.get("d_layerbreak")
        if lb:
            try:
                lb_val = int(lb)
                if self.max_layerbreak_count < 1:
                    self.max_layerbreak_count = 1
                    self.layerbreak_values = [lb_val]
                elif lb_val > (self.layerbreak_values[0] if self.layerbreak_values else 0):
                    self.layerbreak_values = [lb_val]
            except (ValueError, TypeError):
                pass

        self.longest_pvd_hex = _keep_longest(self.longest_pvd_hex, data.get("d_pvd"))
        self.longest_pic_hex = _keep_longest(self.longest_pic_hex, data.get("d_pic_data"))
        self.longest_bca_hex = _keep_longest(self.longest_bca_hex, data.get("d_bca"))
        self.longest_header_hex = _keep_longest(self.longest_header_hex, data.get("d_header"))

        protection = build_protection(data)
        self.longest_protection = _keep_longest(self.longest_protection, protection)

        sbi = data.get("d_libcrypt") or data.get("d_securom") or None
        self.longest_sbi = _keep_longest(self.longest_sbi, sbi)

        if data.get("d_ssranges"):
            ranges = parse_ss_ranges(data["d_ssranges"])
            if len(ranges) > self.max_sector_ranges_count:
                self.max_sector_ranges_count = len(ranges)
                self.sector_ranges_values = list(ranges)
            raw_repr = data["d_ssranges"]
            self.longest_protection_range_repr = _keep_longest(
                self.longest_protection_range_repr, raw_repr)

        pkeys = build_keys(data)
        if pkeys:
            if len(pkeys) > self.max_protection_keys_count:
                self.max_protection_keys_count = len(pkeys)
                self.protection_key_elements = list(pkeys)
            for k in pkeys:
                self.longest_protection_key = _keep_longest(self.longest_protection_key, k)

        self.longest_cue = _keep_longest(self.longest_cue, data.get("d_cue"))

        region_str = data.get("d_region", "")
        region_names = [r.strip() for r in region_str.split(", ")
                        if r.strip() and r.strip() in REGION_NAME_TO_CODE]
        if len(region_names) > self.max_region_count:
            self.max_region_count = len(region_names)
            self.region_codes = [REGION_NAME_TO_CODE[r] for r in region_names]

        lang2_codes = []
        for lang3 in data.get("d_languages[]", []):
            lang2 = LANG3_TO_LANG2.get(lang3)
            if lang2:
                lang2_codes.append(lang2)
        if len(lang2_codes) > self.max_language_count:
            self.max_language_count = len(lang2_codes)
            self.language_codes = list(lang2_codes)

        rings = data.get("rings", [])
        if len(rings) > self.max_ring_entry_count:
            self.max_ring_entry_count = len(rings)

        for ring_obj in rings:
            offset, offset_extra, layers_dict = parse_ring_entry(ring_obj)
            offset_count = int(offset is not None) + int(offset_extra is not None)
            if offset_count > self.max_offset_values_count:
                self.max_offset_values_count = offset_count
            if len(layers_dict) > self.max_ring_layer_count:
                self.max_ring_layer_count = len(layers_dict)
            for layer in layers_dict.values():
                self.longest_mastering_code = _keep_longest(
                    self.longest_mastering_code, layer["mastering_code"])
                self.longest_mastering_sid = _keep_longest(
                    self.longest_mastering_sid, layer["mastering_sid"])
                if len(layer["mould_sids"]) > self.max_mould_sids_count:
                    self.max_mould_sids_count = len(layer["mould_sids"])
                    self.mould_sid_elements = list(layer["mould_sids"])
                for ms in layer["mould_sids"]:
                    self.longest_mould_sid = _keep_longest(self.longest_mould_sid, ms)
                if len(layer["toolstamps"]) > self.max_toolstamps_count:
                    self.max_toolstamps_count = len(layer["toolstamps"])
                    self.toolstamp_elements = list(layer["toolstamps"])
                for ts in layer["toolstamps"]:
                    self.longest_toolstamp = _keep_longest(self.longest_toolstamp, ts)
                if len(layer["additional_moulds"]) > self.max_additional_moulds_count:
                    self.max_additional_moulds_count = len(layer["additional_moulds"])
                    self.additional_mould_elements = list(layer["additional_moulds"])
                for am in layer["additional_moulds"]:
                    self.longest_additional_mould = _keep_longest(
                        self.longest_additional_mould, am)

                self._add_to_pool(self.mastering_code_pool, layer["mastering_code"])
                self._add_to_pool(self.mastering_sid_pool, layer["mastering_sid"])
                for ms in layer["mould_sids"]:
                    self._add_to_pool(self.mould_sid_pool, ms)
                for ts in layer["toolstamps"]:
                    self._add_to_pool(self.toolstamp_pool, ts)
                for am in layer["additional_moulds"]:
                    self._add_to_pool(self.additional_mould_pool, am)

        d_tracks = data.get("d_tracks", "")
        if d_tracks.strip():
            track_hashes = parse_track_hashes(d_tracks)
            if len(track_hashes) > self.max_track_count:
                self.max_track_count = len(track_hashes)
            for size, _, _, _ in track_hashes:
                if size > self.longest_track_size:
                    self.longest_track_size = size

        dumpers = merge_dumpers(data)
        if len(dumpers) > self.max_dumper_count:
            self.max_dumper_count = len(dumpers)
            self.dumper_names = list(dumpers)

        changes = data.get("changes", [])
        if len(changes) > self.max_changes_count:
            self.max_changes_count = len(changes)

        for change in changes:
            fields = change.get("fields", [])
            if len(fields) > self.max_change_fields_count:
                self.max_change_fields_count = len(fields)
            payload = build_submission_data(fields)
            payload_str = json.dumps(payload)
            if self.longest_change_payload is None or len(payload_str) > len(json.dumps(self.longest_change_payload)):
                self.longest_change_payload = payload

            for fentry in payload.get("fields", []):
                fentry_str = json.dumps(fentry)
                if self.longest_change_field_entry is None or len(fentry_str) > len(json.dumps(self.longest_change_field_entry)):
                    self.longest_change_field_entry = fentry

        if len(changes) > len(self.change_payloads):
            self.change_payloads = []
            for change in changes:
                fields = change.get("fields", [])
                payload = build_submission_data(fields)
                self.change_payloads.append({
                    "payload": payload,
                    "date": change.get("date", ""),
                    "user": change.get("user", ""),
                })

    def summary(self):
        lines = [
            f"Files processed: {self.files_processed}",
            f"Files skipped (empty): {self.files_skipped}",
            "",
            f"longest_title:             {len(self.longest_title or '')} chars",
            f"longest_title_foreign:     {len(self.longest_title_foreign or '')} chars",
            f"longest_disc_title:        {len(self.longest_disc_title or '')} chars",
            f"longest_disc_number: {len(self.longest_disc_number or '')} chars",
            f"longest_version:           {len(self.longest_version or '')} chars",
            f"longest_filename_suffix:   {len(self.longest_filename_suffix or '')} chars",
            f"longest_comments:          {len(self.longest_comments or '')} chars",
            f"longest_contents:          {len(self.longest_contents or '')} chars",
            f"longest_exe_date:          {len(self.longest_exe_date or '')} chars",
            "",
            f"max_serial_count:          {self.max_serial_count}",
            f"longest_serial_element:    {len(self.longest_serial_element or '')} chars",
            f"max_edition_count:         {self.max_edition_count}",
            f"longest_edition_element:   {len(self.longest_edition_element or '')} chars",
            f"max_barcode_count:         {self.max_barcode_count}",
            f"longest_barcode_element:   {len(self.longest_barcode_element or '')} chars",
            f"max_error_count:           {self.max_error_count}",
            "",
            f"max_layerbreak_count:      {self.max_layerbreak_count}",
            f"longest_pvd_hex:           {len(self.longest_pvd_hex or '')} chars",
            f"longest_pic_hex:           {len(self.longest_pic_hex or '')} chars",
            f"longest_bca_hex:           {len(self.longest_bca_hex or '')} chars",
            f"longest_header_hex:        {len(self.longest_header_hex or '')} chars",
            "",
            f"longest_protection:        {len(self.longest_protection or '')} chars",
            f"max_sector_ranges:         {self.max_sector_ranges_count}",
            f"longest_sbi:               {len(self.longest_sbi or '')} chars",
            f"max_protection_keys:       {self.max_protection_keys_count}",
            f"longest_protection_key:    {len(self.longest_protection_key or '')} chars",
            f"longest_cue:              {len(self.longest_cue or '')} chars",
            "",
            f"max_region_count:          {self.max_region_count}",
            f"max_language_count:        {self.max_language_count}",
            "",
            f"max_ring_entry_count:      {self.max_ring_entry_count}",
            f"max_ring_layer_count:      {self.max_ring_layer_count}",
            f"longest_mastering_code:    {len(self.longest_mastering_code or '')} chars",
            f"longest_mastering_sid:     {len(self.longest_mastering_sid or '')} chars",
            f"max_mould_sids_count:      {self.max_mould_sids_count}",
            f"longest_mould_sid:         {len(self.longest_mould_sid or '')} chars",
            f"max_toolstamps_count:      {self.max_toolstamps_count}",
            f"longest_toolstamp:         {len(self.longest_toolstamp or '')} chars",
            f"max_additional_moulds:     {self.max_additional_moulds_count}",
            f"longest_additional_mould:  {len(self.longest_additional_mould or '')} chars",
            f"max_offset_values_count:   {self.max_offset_values_count}",
            f"mastering_code_pool:       {len(self.mastering_code_pool)} unique values",
            f"mastering_sid_pool:        {len(self.mastering_sid_pool)} unique values",
            f"mould_sid_pool:            {len(self.mould_sid_pool)} unique values",
            f"toolstamp_pool:            {len(self.toolstamp_pool)} unique values",
            f"additional_mould_pool:     {len(self.additional_mould_pool)} unique values",
            "",
            f"max_track_count:           {self.max_track_count}",
            f"longest_track_size:        {self.longest_track_size}",
            f"max_dumper_count:          {self.max_dumper_count}",
            "",
            f"max_changes_count:         {self.max_changes_count}",
            f"max_change_fields_count:   {self.max_change_fields_count}",
            f"longest_change_payload:    {len(json.dumps(self.longest_change_payload or {}))} chars",
        ]
        return "\n".join(lines)


DISC_ID = 1
SYSTEM_CODE = "MAXTEST"
SYSTEM_NAME = "Max Complexity Test System"
MEDIA_TYPE_CODE = "test4l"
MEDIA_TYPE_NAME = "Max Test (4-layer)"
MEDIA_TYPE_LAYERS = 4
MEDIA_TYPE_ROM_EXT = "bin"
CATEGORY_ID = 1
SUBMITTER_USER_ID = 1


def _pad_list(elements, longest_element, target_count):
    """Pad a list to target_count by repeating elements, each at most as long
    as the longest observed element. Returns a new list."""
    if target_count <= 0:
        return []
    if not elements:
        elements = [longest_element or "x"]
    result = []
    for i in range(target_count):
        result.append(elements[i % len(elements)])
    if longest_element and len(result) > 0:
        result[0] = longest_element
    return result


def generate_sql(stats, output_path):
    with open(output_path, "w") as out:
        out.write("-- Auto-generated max-complexity disc for UI testing\n")
        out.write(f"-- Generated: {datetime.now().astimezone().isoformat()}\n")
        out.write("--\n")
        for line in stats.summary().split("\n"):
            out.write(f"-- {line}\n")
        out.write("\nBEGIN;\n\n")

        # 1. Custom media type with 4 layers
        out.write("-- Custom media type with 4 layers and bin extension\n")
        out.write(f"""INSERT INTO media_types (code, name, layer_count, rom_extension)
VALUES ({sql_str(MEDIA_TYPE_CODE)}, {sql_str(MEDIA_TYPE_NAME)}, {MEDIA_TYPE_LAYERS}, {sql_str(MEDIA_TYPE_ROM_EXT)})
ON CONFLICT (code) DO UPDATE SET
    name = EXCLUDED.name,
    layer_count = EXCLUDED.layer_count,
    rom_extension = EXCLUDED.rom_extension;\n\n""")

        # 2. System with all has_ flags enabled
        out.write("-- System with all has_* flags enabled\n")
        out.write(f"""INSERT INTO systems
    (code, name, media_types,
     has_title_foreign, has_disc_title, has_disc_number,
     has_serial, has_version, has_edition, has_barcode,
     has_error_count, has_exe_date, has_edc,
     has_pvd, has_pic, has_bca, has_header,
     has_protection, has_sector_ranges, has_sbi, has_offset_extra)
VALUES
    ({sql_str(SYSTEM_CODE)}, {sql_str(SYSTEM_NAME)},
     ARRAY[{sql_str(MEDIA_TYPE_CODE)}]::TEXT[],
     TRUE, TRUE, TRUE,
     TRUE, TRUE, TRUE, TRUE,
     TRUE, TRUE, TRUE,
     TRUE, TRUE, TRUE, TRUE,
     TRUE, TRUE, TRUE, TRUE)
ON CONFLICT (code) DO UPDATE SET
    name = EXCLUDED.name,
    media_types = EXCLUDED.media_types,
    has_title_foreign = TRUE, has_disc_title = TRUE, has_disc_number = TRUE,
    has_serial = TRUE, has_version = TRUE, has_edition = TRUE, has_barcode = TRUE,
    has_error_count = TRUE, has_exe_date = TRUE, has_edc = TRUE,
    has_pvd = TRUE, has_pic = TRUE, has_bca = TRUE, has_header = TRUE,
    has_protection = TRUE, has_sector_ranges = TRUE, has_sbi = TRUE, has_offset_extra = TRUE;\n\n""")

        # 2. Ensure a user exists for submissions
        out.write("-- Ensure submitter user exists\n")
        out.write(f"""INSERT INTO users
    (id, username, email, password_hash, role, is_active, email_verified)
    OVERRIDING SYSTEM VALUE
VALUES
    ({SUBMITTER_USER_ID}, 'max_test_user', 'max_test@test.invalid', '!',
     'Admin', TRUE, FALSE)
ON CONFLICT (id) DO NOTHING;\n\n""")

        # 3. Delete existing disc id=1 and dependents
        out.write("-- Remove existing disc id=1 and all dependents\n")
        out.write(f"DELETE FROM disc_submissions WHERE target_disc_id = {DISC_ID};\n")
        out.write(f"DELETE FROM discs WHERE id = {DISC_ID};\n\n")

        # 4. Build synthetic disc values
        title = stats.longest_title or "Max Test Title"
        title_foreign = stats.longest_title_foreign
        disc_title = stats.longest_disc_title
        disc_number = stats.longest_disc_number
        version = stats.longest_version
        filename_suffix = stats.longest_filename_suffix
        comments = stats.longest_comments
        contents = stats.longest_contents
        exe_date = stats.longest_exe_date

        serials = _pad_list(
            stats.serial_elements, stats.longest_serial_element, stats.max_serial_count)
        editions = _pad_list(
            stats.edition_elements, stats.longest_edition_element, stats.max_edition_count)
        barcodes = _pad_list(
            stats.barcode_elements, stats.longest_barcode_element, stats.max_barcode_count)
        error_count = stats.max_error_count

        layerbreaks = stats.layerbreak_values if stats.layerbreak_values else None

        pvd_bytes = None
        if stats.longest_pvd_hex:
            raw = parse_hex_dump_with_address(stats.longest_pvd_hex)
            pvd_bytes = raw[:82] if len(raw) > 82 else raw

        pic_bytes = None
        if stats.longest_pic_hex:
            pic_bytes = parse_hex_raw_spaced(stats.longest_pic_hex)

        bca_bytes = None
        if stats.longest_bca_hex:
            bca_bytes = parse_hex_bca(stats.longest_bca_hex)

        header_bytes = None
        if stats.longest_header_hex:
            header_bytes = parse_hex_dump_with_address(stats.longest_header_hex)

        protection = stats.longest_protection
        sbi = stats.longest_sbi

        sector_ranges = stats.sector_ranges_values if stats.sector_ranges_values else None
        keys = _pad_list(
            stats.protection_key_elements, stats.longest_protection_key,
            stats.max_protection_keys_count) if stats.max_protection_keys_count > 0 else None

        cue = stats.longest_cue

        out.write("-- Insert max-complexity disc\n")
        out.write(f"""INSERT INTO discs
    (id, enabled, media_type_code, category_id, system_code, title,
     filename_suffix, comments, contents,
     title_foreign, disc_title, disc_number,
     serial, version, edition, barcode, error_count,
     exe_date, edc, layerbreaks,
     pvd, pic, bca, header,
     protection, sector_ranges, sbi, keys,
     cue, questionable)
    OVERRIDING SYSTEM VALUE
VALUES
    ({DISC_ID}, TRUE, {sql_str(MEDIA_TYPE_CODE)}, {CATEGORY_ID}, {sql_str(SYSTEM_CODE)},
     {sql_str(title)},
     {sql_str_or_null(filename_suffix)},
     {sql_str_or_null(comments)},
     {sql_str_or_null(contents)},
     {sql_str_or_null(title_foreign)},
     {sql_str_or_null(disc_title)},
     {sql_str_or_null(disc_number)},
     {sql_text_array(serials)},
     {sql_str_or_null(version)},
     {sql_text_array(editions)},
     {sql_text_array(barcodes)},
     {sql_int(error_count)},
     {sql_str_or_null(exe_date)},
     TRUE,
     {sql_int_array(layerbreaks) if layerbreaks else "NULL"},
     {sql_bytea(pvd_bytes)},
     {sql_bytea(pic_bytes)},
     {sql_bytea(bca_bytes)},
     {sql_bytea(header_bytes)},
     {sql_str_or_null(protection)},
     {sql_int4range_array(sector_ranges) if sector_ranges else "NULL"},
     {sql_str_or_null(sbi)},
     {sql_text_array(keys) if keys else "NULL"},
     {sql_str_or_null(cue)},
     FALSE);\n\n""")

        # 5. Regions
        if stats.region_codes:
            out.write(f"-- Regions ({len(stats.region_codes)} rows)\n")
            out.write("INSERT INTO disc_regions (disc_id, region_code) VALUES\n")
            vals = [f"    ({DISC_ID}, '{rc}')" for rc in stats.region_codes]
            out.write(",\n".join(vals))
            out.write(";\n\n")

        # 6. Languages
        if stats.language_codes:
            out.write(f"-- Languages ({len(stats.language_codes)} rows)\n")
            out.write("INSERT INTO disc_languages (disc_id, language_code) VALUES\n")
            vals = [f"    ({DISC_ID}, '{lc}')" for lc in stats.language_codes]
            out.write(",\n".join(vals))
            out.write(";\n\n")

        # 7. Ring code entries with 4 layers each, using diverse pool values
        target_layers = 4
        ring_entry_count = max(stats.max_ring_entry_count, 1)
        out.write(f"-- Ring code entries ({ring_entry_count} entries x {target_layers} layers)\n")

        mc_pool = sorted(stats.mastering_code_pool, key=len, reverse=True) or [stats.longest_mastering_code or ""]
        ms_pool = sorted(stats.mastering_sid_pool, key=len, reverse=True) or [stats.longest_mastering_sid or ""]
        mo_pool = sorted(stats.mould_sid_pool, key=len, reverse=True) or [stats.longest_mould_sid or "x"]
        ts_pool = sorted(stats.toolstamp_pool, key=len, reverse=True) or [stats.longest_toolstamp or "x"]
        am_pool = sorted(stats.additional_mould_pool, key=len, reverse=True) or [stats.longest_additional_mould or "x"]

        slot = 0
        for entry_idx in range(ring_entry_count):
            offset_val = entry_idx if stats.max_offset_values_count > 0 else None
            offset_extra = (entry_idx + 1) if stats.max_offset_values_count > 1 else None
            out.write(f"""INSERT INTO disc_ring_code_entries (disc_id, offset_value, offset_extra_value, sample_data_start, comment)
VALUES ({DISC_ID}, {sql_int(offset_val)}, {sql_int(offset_extra)}, NULL, {sql_str(f'entry {entry_idx + 1}')});\n""")

            for layer_num in range(target_layers):
                mastering_code = mc_pool[slot % len(mc_pool)]
                mastering_sid = ms_pool[slot % len(ms_pool)]

                mould_sids = []
                for j in range(stats.max_mould_sids_count):
                    mould_sids.append(mo_pool[(slot + j) % len(mo_pool)])
                toolstamps = []
                for j in range(stats.max_toolstamps_count):
                    toolstamps.append(ts_pool[(slot + j) % len(ts_pool)])
                additional_moulds = []
                for j in range(stats.max_additional_moulds_count):
                    additional_moulds.append(am_pool[(slot + j) % len(am_pool)])

                slot += 1

                out.write(f"""INSERT INTO disc_ring_code_layers
    (entry_id, layer, mastering_code, mastering_sid, mould_sids, toolstamps, additional_moulds)
VALUES
    (currval('disc_ring_code_entries_id_seq'), {layer_num},
     {sql_str_or_null(mastering_code)},
     {sql_str_or_null(mastering_sid)},
     {sql_text_array(mould_sids)},
     {sql_text_array(toolstamps)},
     {sql_text_array(additional_moulds)});\n""")
            out.write("\n")

        # 8. Files (tracks)
        track_count = max(stats.max_track_count, 1)
        out.write(f"-- Files ({track_count} tracks + whole-disc + cue)\n")
        fake_size = stats.longest_track_size if stats.longest_track_size > 0 else 700000000
        d32 = "d" * 32
        a40 = "a" * 40
        b32 = "b" * 32
        c40 = "c" * 40
        out.write(
            f"INSERT INTO files (disc_id, track_number, size, crc32, md5, sha1) VALUES\n"
            f"    ({DISC_ID}, '0', {fake_size}, 'deadbeef', '{d32}', '{a40}');\n"
        )
        for t in range(1, track_count + 1):
            crc = f"{t:08x}"[:8]
            hx = f"{t % 256:02x}"
            md5 = hx * 16
            sha1 = hx * 20
            per_track_size = fake_size // track_count
            out.write(
                f"INSERT INTO files (disc_id, track_number, size, crc32, md5, sha1) VALUES\n"
                f"    ({DISC_ID}, '{t}', {per_track_size}, '{crc}', '{md5}', '{sha1}');\n"
            )
        out.write(
            f"INSERT INTO files (disc_id, track_number, size, crc32, md5, sha1) VALUES\n"
            f"    ({DISC_ID}, NULL, 1024, 'cafebabe', '{b32}', '{c40}');\n\n"
        )

        # 9. Dumpers
        if stats.dumper_names:
            out.write(f"-- Dumpers ({len(stats.dumper_names)} users)\n")
            for i, dumper_name in enumerate(stats.dumper_names):
                uid = i + 2  # user id 1 is reserved for max_test_user
                out.write(f"""INSERT INTO users (id, username, email, password_hash, role, is_active, email_verified)
    OVERRIDING SYSTEM VALUE
VALUES ({uid}, {sql_str(dumper_name)}, {sql_str(f'{dumper_name}@imported.invalid')}, '!', 'User', FALSE, FALSE)
ON CONFLICT (id) DO NOTHING;\n""")
            out.write("\n")
            out.write("INSERT INTO disc_dumpers (disc_id, user_id) VALUES\n")
            vals = [f"    ({DISC_ID}, {i + 2})" for i in range(len(stats.dumper_names))]
            out.write(",\n".join(vals))
            out.write(";\n\n")

        # 10. Disc submissions (changes)
        if stats.max_changes_count > 0:
            out.write(f"-- Disc submissions ({stats.max_changes_count} Approved rows)\n")
            out.write("INSERT INTO disc_submissions\n")
            out.write("    (submission_type, submitter_id, target_disc_id, changes, status,\n")
            out.write("     reviewer_id, review_comment, created_at, reviewed_at)\nVALUES\n")

            submission_vals = []
            for i in range(stats.max_changes_count):
                if i < len(stats.change_payloads):
                    payload = stats.change_payloads[i]["payload"]
                else:
                    payload = stats.longest_change_payload or {"fields": []}
                # use the largest payload for slot 0
                if i == 0 and stats.longest_change_payload:
                    payload = stats.longest_change_payload
                ts = f"2025-01-{(i % 28) + 1:02d} 12:00:00"
                submission_vals.append(
                    f"    ('Edit', {SUBMITTER_USER_ID}, {DISC_ID}, "
                    f"{sql_jsonb(payload)}, "
                    f"'Approved', {SUBMITTER_USER_ID}, {sql_str('max test')}, "
                    f"{sql_str(ts)}, {sql_str(ts)})"
                )
            out.write(",\n".join(submission_vals))
            out.write(";\n\n")

        # 11. Fix sequences
        out.write("-- Reset sequences\n")
        out.write("SELECT setval('discs_id_seq', (SELECT COALESCE(MAX(id), 1) FROM discs));\n")
        out.write("SELECT setval('users_id_seq', (SELECT COALESCE(MAX(id), 1) FROM users));\n")
        out.write("SELECT setval('disc_ring_code_entries_id_seq', (SELECT COALESCE(MAX(id), 1) FROM disc_ring_code_entries));\n")
        out.write("SELECT setval('disc_ring_code_layers_id_seq', (SELECT COALESCE(MAX(id), 1) FROM disc_ring_code_layers));\n")
        out.write("SELECT setval('files_id_seq', (SELECT COALESCE(MAX(id), 1) FROM files));\n")
        out.write("SELECT setval('disc_submissions_id_seq', (SELECT COALESCE(MAX(id), 1) FROM disc_submissions));\n")

        out.write("\nCOMMIT;\n")

    print(f"Wrote {output_path}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(
        description="Generate max-complexity disc SQL from scraped Redump data")
    parser.add_argument("--data-dir", default="data/redump/db",
                        help="Path to scraped JSON directory")
    parser.add_argument("--output", "-o", default="max_disc_id1.sql",
                        help="Output SQL file path")
    args = parser.parse_args()

    if not os.path.isdir(args.data_dir):
        print(f"Error: {args.data_dir} is not a directory", file=sys.stderr)
        sys.exit(1)

    stats = MaxStats()

    data_dir = args.data_dir
    filenames = sorted(f for f in os.listdir(data_dir) if f.endswith(".json"))
    total = len(filenames)
    print(f"Scanning {total} JSON files in {data_dir} ...", file=sys.stderr)

    for idx, fname in enumerate(filenames):
        path = os.path.join(data_dir, fname)
        if os.path.getsize(path) == 0:
            stats.files_skipped += 1
            continue
        try:
            with open(path) as f:
                data = json.load(f)
            stats.ingest(data)
        except (json.JSONDecodeError, Exception) as e:
            stats.files_skipped += 1
            continue

        if (idx + 1) % 10000 == 0:
            print(f"  ... {idx + 1}/{total}", file=sys.stderr)

    print(file=sys.stderr)
    print(stats.summary(), file=sys.stderr)
    print(file=sys.stderr)

    os.makedirs(os.path.dirname(args.output) or ".", exist_ok=True)
    generate_sql(stats, args.output)


if __name__ == "__main__":
    main()
