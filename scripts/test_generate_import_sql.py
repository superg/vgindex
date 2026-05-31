import io
import unittest

import generate_import_sql


class GenerateImportSqlTests(unittest.TestCase):
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
