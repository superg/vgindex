import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import scraper


class CliTests(unittest.TestCase):
    def test_check_modified_requires_positive_integer(self):
        parser = scraper.build_arg_parser()

        self.assertIsNone(parser.parse_args([]).check_modified)
        self.assertEqual(parser.parse_args(["--check-modified", "200"]).check_modified, 200)

        invalid_args = [
            ["--check-modified"],
            ["--check-modified", "0"],
            ["--check-modified", "-1"],
            ["--check-modified", "abc"],
        ]
        for argv in invalid_args:
            with self.subTest(argv=argv):
                with contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit):
                        parser.parse_args(argv)


class ModifiedReplacementTests(unittest.TestCase):
    def test_modified_phase_moves_first_n_unique_local_files_to_backup(self):
        with tempfile.TemporaryDirectory() as tmp:
            output_dir = Path(tmp)
            json_path = output_dir / "000101.json"
            missing_path = output_dir / "000102.json"
            marker_path = output_dir / "000103.json"
            untouched_path = output_dir / "000104.json"

            json_path.write_text('{"disc_id": 101}', encoding="utf-8")
            marker_path.touch()
            untouched_path.write_text('{"disc_id": 104}', encoding="utf-8")

            config = scraper.ScraperConfig(
                config_path=output_dir / "scraper.cfg",
                last_known_disc_id=200,
                cookie="",
                delay_seconds=0,
                output_dir=str(output_dir),
            )

            with mock.patch.object(
                scraper,
                "iter_modified_disc_ids",
                return_value=iter([101, 102, 101, 103, 104]),
            ) as iter_modified_disc_ids:
                with contextlib.redirect_stdout(io.StringIO()) as stdout:
                    scraper.run_modified_detection_phase(object(), config, 3)

            iter_modified_disc_ids.assert_called_once_with(mock.ANY, 0)
            self.assertFalse(json_path.exists())
            self.assertFalse(missing_path.exists())
            self.assertFalse(marker_path.exists())
            self.assertTrue(untouched_path.exists())

            backup_dirs = [
                path for path in output_dir.iterdir()
                if path.is_dir() and path.name.startswith("backup-")
            ]
            self.assertEqual(len(backup_dirs), 1)
            backup_dir = backup_dirs[0]
            self.assertEqual(
                (backup_dir / "000101.json").read_text(encoding="utf-8"),
                '{"disc_id": 101}',
            )
            self.assertTrue((backup_dir / "000103.json").exists())
            self.assertEqual((backup_dir / "000103.json").stat().st_size, 0)
            self.assertFalse((backup_dir / "000102.json").exists())
            self.assertFalse((backup_dir / "000104.json").exists())
            self.assertIn(
                "Moving local files for 3 latest modified disc ID(s)",
                stdout.getvalue(),
            )


if __name__ == "__main__":
    unittest.main()
