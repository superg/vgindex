import io
import unittest

import generate_import_sql


class GenerateImportSqlTests(unittest.TestCase):
    def test_sanitize_filename_legacy_ascii_substitution_table(self):
        cases = [
            ("é", "e"),
            ("Ś", "S"),
            ("ä", "ae"),
            ("ö", "oe"),
            ("ó", "o"),
            ("ü", "ue"),
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
            ("Ä", "Ae"),
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
            ("Ö", "Oe"),
            ("Ü", "Ue"),
            ("î", "i"),
            ("ô", "o"),
            ("ù", "u"),
            ("Ō", "Oo"),
            ("α", "Alpha"),
            ("û", "u"),
            ("Ú", "U"),
            ("½", "1-2"),
            ("ū", "uu"),
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
            ("ō", "oo"),
            ("Ş", "S"),
            ("ș", "s"),
        ]

        for value, expected in cases:
            with self.subTest(value=value):
                self.assertEqual(generate_import_sql.sanitize_filename(value), expected)

    def test_sanitize_filename_anyascii_and_fallback_behavior(self):
        self.assertEqual(generate_import_sql.sanitize_filename("éßłæȘ"), "esslaeS")
        self.assertEqual(generate_import_sql.sanitize_filename("u\u0308"), "ue")
        self.assertEqual(generate_import_sql.sanitize_filename("Foo\ue000Bar"), "Foo-Bar")

    def test_sanitize_filename_slash_spacing(self):
        self.assertEqual(generate_import_sql.sanitize_filename("Foo / Bar"), "Foo & Bar")
        self.assertEqual(generate_import_sql.sanitize_filename("Foo/Bar"), "Foo-Bar")

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


if __name__ == "__main__":
    unittest.main()
