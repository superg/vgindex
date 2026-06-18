import io
import json
import os
import re
import tempfile
import unittest

import generate_import_sql


class GenerateImportSqlTests(unittest.TestCase):
    def test_seed_data_defines_all_non_synthetic_system_codes(self):
        repo_root = os.path.dirname(os.path.dirname(__file__))
        seed_path = os.path.join(repo_root, "migrations", "002_seed_data.sql")
        with open(seed_path) as f:
            seed_sql = f.read()

        seeded_codes = set(re.findall(r"\('([^']+)',\s*'[^']*',\s*'[^']*',", seed_sql))
        mapped_codes = set(generate_import_sql.SYSTEM_NAME_TO_CODE.values())
        mapped_codes.remove(generate_import_sql.SYNTHETIC_SYSTEM_CODE)

        self.assertEqual(mapped_codes - seeded_codes, set())

    def _write_minimal_disc(self, data_dir, disc_id=1, title="Real Disc 1"):
        path = os.path.join(data_dir, f"{disc_id:06d}.json")
        with open(path, "w") as f:
            json.dump({
                "d_status": "5",
                "system": "Sony PlayStation",
                "media": "CD",
                "d_category": "Games",
                "d_title": title,
            }, f)

    def _generate_import_sql(self, *, include_max_complexity_disc=False):
        with tempfile.TemporaryDirectory() as temp_dir:
            data_dir = os.path.join(temp_dir, "db")
            os.mkdir(data_dir)
            self._write_minimal_disc(data_dir)
            output_path = os.path.join(temp_dir, "import.sql")

            generate_import_sql.process_all(
                data_dir,
                output_path,
                reset=False,
                include_max_complexity_disc=include_max_complexity_disc,
            )

            with open(output_path) as f:
                return f.read()

    def test_process_all_does_not_emit_max_complexity_sql_by_default(self):
        sql = self._generate_import_sql()

        self.assertIn("E'Real Disc 1'", sql)
        self.assertNotIn("MAXTEST", sql)
        self.assertNotIn("test4l", sql)
        self.assertNotIn("Max Complexity Test System", sql)

    def test_process_all_can_emit_max_complexity_sql_when_requested(self):
        sql = self._generate_import_sql(include_max_complexity_disc=True)

        self.assertIn("MAXTEST", sql)
        self.assertIn("test4l", sql)
        self.assertIn("Max Complexity Test System", sql)

    def test_sanitize_filename_ascii_substitution_and_transliteration_table(self):
        cases = [
            ("é", "e"),
            ("Ś", "S"),
            ("ä", "a"),
            ("ö", "o"),
            ("ó", "o"),
            ("ü", "u"),
            ("ł", "l"),
            ("·", "-"),
            ("å", "a"),
            ("ę", "e"),
            ("á", "a"),
            ("ß", "ss"),
            ("ñ", "n"),
            ("â", "a"),
            ("è", "e"),
            ("í", "i"),
            ("ś", "s"),
            ("à", "a"),
            ("ż", "z"),
            ("²", "^2"),
            ("É", "E"),
            ("ç", "c"),
            ("ě", "e"),
            ("ń", "n"),
            ("ë", "e"),
            ("Ä", "A"),
            ("ą", "a"),
            ("ê", "e"),
            ("č", "c"),
            ("ź", "z"),
            ("³", "^3"),
            ("æ", "ae"),
            ("ú", "u"),
            ("ø", "o"),
            ("ć", "c"),
            ("ý", "y"),
            ("ã", "a"),
            ("ò", "o"),
            ("ï", "i"),
            ("õ", "o"),
            ("Ö", "O"),
            ("Ü", "U"),
            ("î", "i"),
            ("ô", "o"),
            ("ù", "u"),
            ("Ō", "O"),
            ("α", "Alpha"),
            ("û", "u"),
            ("Ú", "U"),
            ("½", "1-2"),
            ("ū", "u"),
            ("À", "A"),
            ("Ł", "L"),
            ("È", "E"),
            ("Ø", "O"),
            ("ş", "s"),
            ("ÿ", "y"),
            ("Č", "C"),
            ("Ż", "Z"),
            ("Ș", "S"),
            ("Δ", "Delta"),
            ("μ", "Mu"),
            ("Í", "I"),
            ("Î", "I"),
            ("ì", "i"),
            ("ō", "o"),
            ("Ş", "S"),
            ("ș", "s"),
            ("#", ""),
        ]

        for value, expected in cases:
            with self.subTest(value=value):
                self.assertEqual(generate_import_sql.sanitize_filename(value), expected)

    def test_sanitize_filename_anyascii_and_fallback_behavior(self):
        self.assertEqual(generate_import_sql.sanitize_filename("éßłæȘ"), "esslaeS")
        self.assertEqual(generate_import_sql.sanitize_filename("u\u0308"), "u")
        self.assertEqual(generate_import_sql.sanitize_filename("Foo\ue000Bar"), "Foo-Bar")

    def test_sanitize_filename_colon_spacing_and_hash(self):
        self.assertEqual(generate_import_sql.sanitize_filename("Foo : Bar"), "Foo - Bar")
        self.assertEqual(generate_import_sql.sanitize_filename("Game #1"), "Game 1")

    def test_sanitize_filename_slash_spacing(self):
        self.assertEqual(generate_import_sql.sanitize_filename("Foo / Bar"), "Foo & Bar")
        self.assertEqual(generate_import_sql.sanitize_filename("Foo/Bar"), "Foo-Bar")

    def test_build_rom_base_name_sanitizes_components_before_assembly(self):
        self.assertEqual(
            generate_import_sql.build_rom_base_name(
                "Active Simulation War Daiva Chronicle Re:",
                ["Japan"],
                [],
                None,
                None,
                None,
            ),
            "Active Simulation War Daiva Chronicle Re- (Japan)",
        )

    def test_build_rom_base_name_sanitizes_each_parenthetical_component(self):
        self.assertEqual(
            generate_import_sql.build_rom_base_name(
                "Foo: Bar",
                ["USA / Europe"],
                ["en", "fr"],
                "1:",
                "Label: Test",
                "#Special?",
            ),
            "Foo - Bar (USA & Europe) (En,Fr) (Disc 1-) (Label - Test) (Special)",
        )

    def test_build_rom_base_name_omits_components_that_sanitize_empty(self):
        self.assertEqual(
            generate_import_sql.build_rom_base_name(
                "Game",
                ["#"],
                ["en", "#"],
                "#",
                "?",
                "°",
            ),
            "Game",
        )

    def test_write_users_outputs_only_id_and_username(self):
        out = io.StringIO()

        generate_import_sql._write_users(out, {"Alice": 2, "Bob": 1})

        sql = out.getvalue()
        self.assertIn("INSERT INTO users (id, username) OVERRIDING SYSTEM VALUE", sql)
        self.assertIn("(1, E'Bob')", sql)
        self.assertIn("(2, E'Alice')", sql)
        self.assertNotIn("password", sql)
        self.assertNotIn("email", sql)
        self.assertNotIn("role", sql)

    def test_sql_int4range_array_preserves_scraped_end_values(self):
        sql = generate_import_sql.sql_int4range_array([(108976, 113071), (3719856, 3723951)])

        self.assertEqual(
            sql,
            "ARRAY['[108976,113071)'::INT4RANGE, '[3719856,3723951)'::INT4RANGE]",
        )
        self.assertNotIn("113071]", sql)
        self.assertNotIn("3723951]", sql)


if __name__ == "__main__":
    unittest.main()
