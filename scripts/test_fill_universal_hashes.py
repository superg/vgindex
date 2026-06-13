import pathlib
import sys
import unittest


sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import fill_universal_hashes


HASH_A = "bc3ba08d07787316b494a11ea0da59a5c689c797"
HASH_B = "5bff702c86d8f12e0f062687e9afdbb1f9c94777"


class UniversalHashParserTests(unittest.TestCase):
    def test_extracts_bold_label_with_sha1_colon_inside_tag(self):
        parsed = fill_universal_hashes.parse_universal_hash(
            f"<b>Universal Hash (SHA-1):</b> {HASH_A.upper()}"
        )

        self.assertEqual(parsed.status, "valid")
        self.assertEqual(parsed.hash_hex, HASH_A)

    def test_extracts_bold_label_with_sha1_colon_after_tag(self):
        parsed = fill_universal_hashes.parse_universal_hash(
            f"<b>Universal Hash (SHA-1)</b>: {HASH_A}"
        )

        self.assertEqual(parsed.status, "valid")
        self.assertEqual(parsed.hash_hex, HASH_A)

    def test_extracts_label_without_sha1(self):
        parsed = fill_universal_hashes.parse_universal_hash(f"Universal Hash: {HASH_A}")

        self.assertEqual(parsed.status, "valid")
        self.assertEqual(parsed.hash_hex, HASH_A)

    def test_extracts_sha1_without_dash(self):
        parsed = fill_universal_hashes.parse_universal_hash(f"Universal Hash (SHA1): {HASH_A}")

        self.assertEqual(parsed.status, "valid")
        self.assertEqual(parsed.hash_hex, HASH_A)

    def test_rejects_non_40_character_value(self):
        parsed = fill_universal_hashes.parse_universal_hash("Universal Hash (SHA-1): +8385")

        self.assertEqual(parsed.status, "malformed")
        self.assertIsNone(parsed.hash_hex)

    def test_rejects_41_character_hex_value(self):
        parsed = fill_universal_hashes.parse_universal_hash(f"Universal Hash: {HASH_A}f")

        self.assertEqual(parsed.status, "malformed")
        self.assertIsNone(parsed.hash_hex)

    def test_rejects_non_hex_value(self):
        parsed = fill_universal_hashes.parse_universal_hash(
            "Universal Hash: gggggggggggggggggggggggggggggggggggggggg"
        )

        self.assertEqual(parsed.status, "malformed")
        self.assertIsNone(parsed.hash_hex)

    def test_multiple_conflicting_hashes_are_ambiguous(self):
        parsed = fill_universal_hashes.parse_universal_hash(
            f"Universal Hash: {HASH_A} Universal Hash: {HASH_B}"
        )

        self.assertEqual(parsed.status, "ambiguous")
        self.assertEqual(parsed.matches, (HASH_B, HASH_A))


class UniversalHashPlanTests(unittest.TestCase):
    def test_build_update_plan_classifies_dry_run_actions(self):
        rows = [
            (1, f"Universal Hash: {HASH_A}", None),
            (2, f"Universal Hash: {HASH_A}", bytes.fromhex(HASH_A)),
            (3, f"Universal Hash: {HASH_A}", bytes.fromhex(HASH_B)),
            (4, "Universal Hash (SHA-1): +8385", None),
            (5, f"Universal Hash: {HASH_A} Universal Hash: {HASH_B}", None),
        ]

        plan = fill_universal_hashes.build_update_plan(rows)

        self.assertEqual(plan.scanned, 5)
        self.assertEqual(plan.extracted, 3)
        self.assertEqual(plan.unchanged, 1)
        self.assertEqual([update.disc_id for update in plan.updates], [1])
        self.assertEqual([row.disc_id for row in plan.conflicts], [3])
        self.assertEqual([row.disc_id for row in plan.malformed], [4])
        self.assertEqual([row.disc_id for row in plan.ambiguous], [5])

    def test_overwrite_turns_conflict_into_replacement(self):
        rows = [(1, f"Universal Hash: {HASH_A}", bytes.fromhex(HASH_B))]

        plan = fill_universal_hashes.build_update_plan(rows, overwrite=True)

        self.assertEqual(len(plan.conflicts), 0)
        self.assertEqual(len(plan.updates), 1)
        self.assertTrue(plan.updates[0].replacing_existing)
        self.assertEqual(plan.updates[0].hash_bytes, bytes.fromhex(HASH_A))


if __name__ == "__main__":
    unittest.main()
