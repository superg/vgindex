import contextlib
import io
import tempfile
import unittest
from pathlib import Path

import scraper


class ParserTests(unittest.TestCase):
    def test_parse_index_page(self):
        html = """
        <p id="welcome"><span>Logged in as <strong>Tester</strong>.</span></p>
        <div class="main-head"><h2 class="hn"><span>Redump</span></h2></div>
        <div id="category1" class="main-content main-category">
          <div id="forum11" class="main-item odd">
            <div class="item-subject"><h3 class="hn"><a href="/forum/11/new-dumps/">New Dumps</a></h3></div>
          </div>
          <div id="forum12" class="main-item even redirect">
            <div class="item-subject"><h3 class="hn"><a href="https://example.test/">External</a></h3></div>
          </div>
        </div>
        """
        forums = scraper.parse_index_page(html, "http://forum.redump.org/")
        self.assertEqual(len(forums), 1)
        self.assertEqual(forums[0].forum_id, 11)
        self.assertEqual(forums[0].forum_name, "New Dumps")
        self.assertEqual(forums[0].category_name, "Redump")
        self.assertEqual(forums[0].source_url, "http://forum.redump.org/forum/11/new-dumps/")

    def test_parse_forum_page_excludes_counts_and_keeps_status(self):
        forum = scraper.ForumInfo(11, "New Dumps", "Redump", "http://forum.redump.org/forum/11/new-dumps/")
        html = """
        <link rel="next" href="/forum/11/new-dumps/page/2/" />
        <div id="forum11" class="main-content main-forum">
          <div id="topic100" class="main-item odd sticky closed">
            <div class="item-subject">
              <h3 class="hn"><span class="item-num">1</span>
                <span class="item-status">Sticky, Closed</span>
                <a href="/topic/100/example/">Example</a>
              </h3>
            </div>
            <ul class="item-info">
              <li class="info-replies"><strong>4</strong></li>
              <li class="info-views"><strong>1,234</strong></li>
            </ul>
          </div>
          <div id="topic101" class="main-item even moved">
            <div class="item-subject">
              <h3 class="hn"><a href="/topic/200/target/">Moved example</a></h3>
            </div>
          </div>
        </div>
        """
        topics, next_url = scraper.parse_forum_page(html, forum.source_url, forum)
        self.assertEqual(next_url, "http://forum.redump.org/forum/11/new-dumps/page/2/")
        self.assertEqual(topics[0].topic_id, 100)
        self.assertTrue(topics[0].flags["sticky"])
        self.assertTrue(topics[0].flags["closed"])
        self.assertFalse(topics[0].flags["moved"])
        self.assertEqual(topics[0].view_count, 1234)
        self.assertEqual(topics[1].moved_to_topic_id, 200)
        record = scraper.base_topic_record(topics[0])
        self.assertNotIn("view_count", record)
        metadata_record = scraper.topic_summary_record(topics[0])
        self.assertEqual(metadata_record["view_count"], 1234)
        self.assertNotIn("reply_count", record)
        self.assertNotIn("post_count", record)

    def test_parse_topic_page_preserves_message_html_and_strips_signature(self):
        html = """
        <h2 class="hn"><span>Posts: 1 to 1 of 1</span></h2>
        <div class="main-content main-topic">
          <div class="post odd firstpost">
            <div id="p123" class="posthead">
              <h3 class="hn post-ident">
                <span class="post-byline"><span>Topic by </span><a href="/user/44/">Alice</a></span>
                <span class="post-link"><a class="permalink" href="/post/123/#p123">2020-01-02 03:04:05</a></span>
                <span class="post-edit">(edited by Alice 2020-01-03 04:05:06)</span>
              </h3>
            </div>
            <div class="postbody">
              <div class="post-author"><ul class="author-ident"><li class="username"><a href="/user/44/">Alice</a></li></ul></div>
              <div class="post-entry">
                <div class="entry-content">
                  <p>Hello <strong>world</strong></p>
                  <blockquote><p>quoted</p></blockquote>
                  <div class="post-attachments">
                    <p>Post's attachments</p>
                    <a href="/misc.php?action=pun_attachment&amp;item=789&amp;download=1">dump.log</a>
                  </div>
                  <div class="sig-content"><p>signature</p></div>
                </div>
              </div>
            </div>
          </div>
        </div>
        """
        posts, attachments, next_url, expected_total = scraper.parse_topic_page(
            html, "http://forum.redump.org/topic/100/example/"
        )
        self.assertIsNone(next_url)
        self.assertEqual(expected_total, 1)
        self.assertEqual(len(posts), 1)
        post = posts[0]
        self.assertEqual(post["post_id"], 123)
        self.assertEqual(post["author_name"], "Alice")
        self.assertEqual(post["posted_at"], "2020-01-02 03:04:05")
        self.assertEqual(post["edited_by"], "Alice")
        self.assertIn("<strong>world</strong>", post["message_html"])
        self.assertIn("<blockquote>", post["message_html"])
        self.assertNotIn("signature", post["message_html"])
        self.assertNotIn("pun_attachment", post["message_html"])
        self.assertNotIn("plain_text", post)
        self.assertNotIn("author_id", post)
        self.assertEqual(post["attachment_ids"], ["789"])
        self.assertEqual(attachments[0]["attachment_id"], "789")
        self.assertEqual(attachments[0]["filename"], "dump.log")

    def test_parse_message_html_handles_nested_attachment_links(self):
        html = """
        <div class="post">
          <div id="p123" class="posthead"></div>
          <div class="post-entry">
            <div class="entry-content">
              <p>before</p>
              <div class="post-attachments">
                <a href="/misc.php?action=pun_attachment&amp;item=1&amp;download=1">
                  <span><a href="/misc.php?action=pun_attachment&amp;item=2&amp;download=1">nested.log</a></span>
                </a>
              </div>
              <p>after</p>
            </div>
          </div>
        </div>
        """
        soup = scraper.BeautifulSoup(html, "lxml")
        message_html = scraper.parse_message_html(soup.select_one(".post"))
        self.assertIn("before", message_html)
        self.assertIn("after", message_html)
        self.assertNotIn("pun_attachment", message_html)

    def test_parse_short_posts_total(self):
        soup = scraper.BeautifulSoup('<h2 class="hn"><span>Posts: 9</span></h2>', "lxml")
        self.assertEqual(scraper.parse_items_total(soup), 9)

    def test_parse_topic_metadata_from_direct_topic_page(self):
        forums_by_id = {
            9: scraper.ForumInfo(9, "Staff", "Redump", "http://forum.redump.org/forum/9/staff/")
        }
        html = """
        <title>Example topic - Redump Forum</title>
        <h1 class="main-title">[ Closed ] <a class="permalink" href="/topic/123/example/">Example topic</a></h1>
        <div id="brd-crumbs-top" class="crumbs">
          <p><a href="/">Redump Forum</a> <a href="/forum/9/staff/">Staff</a> <span>Example topic</span></p>
        </div>
        <div id="forum9" class="main-content main-topic"></div>
        """
        record = scraper.parse_topic_metadata(
            html,
            "http://forum.redump.org/viewtopic.php?id=123",
            123,
            forums_by_id,
        )
        self.assertEqual(record["topic_id"], 123)
        self.assertEqual(record["category_name"], "Redump")
        self.assertEqual(record["forum_id"], 9)
        self.assertEqual(record["forum_name"], "Staff")
        self.assertEqual(record["subject"], "Example topic")
        self.assertEqual(record["flags"], {"closed": True})

    def test_validate_auth(self):
        username = scraper.validate_auth('<p id="welcome"><span>Logged in as <strong>Tester</strong>.</span></p>')
        self.assertEqual(username, "Tester")
        with self.assertRaises(scraper.ScrapeError):
            scraper.validate_auth('<p id="welcome"><span>Not logged in.</span></p>')

class ConfigTests(unittest.TestCase):
    def test_load_config(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scraper.cfg"
            path.write_text(
                "[scraper]\n"
                "base_url = http://forum.redump.org\n"
                "cookie = PHPSESSID=abc\n"
                "max_known_topic_id = 123\n"
                "missing_stop_after = 25\n"
                "delay_seconds = 0.2\n"
                "workers = 2\n"
                "output_dir = data/redump/forum\n",
                encoding="utf-8",
            )
            config = scraper.load_config(path)
            self.assertEqual(config.cookie, "PHPSESSID=abc")
            self.assertEqual(config.max_known_topic_id, 123)
            self.assertEqual(config.missing_stop_after, 25)
            self.assertEqual(config.delay_seconds, 0.2)
            self.assertEqual(config.workers, 2)

    def test_missing_cookie_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scraper.cfg"
            path.write_text("[scraper]\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                scraper.load_config(path)

    def test_invalid_workers_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scraper.cfg"
            path.write_text("[scraper]\ncookie = PHPSESSID=abc\nworkers = 0\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                scraper.load_config(path)

    def test_invalid_missing_stop_after_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scraper.cfg"
            path.write_text("[scraper]\ncookie = PHPSESSID=abc\nmissing_stop_after = 0\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                scraper.load_config(path)

    def test_invalid_max_known_topic_id_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scraper.cfg"
            path.write_text("[scraper]\ncookie = PHPSESSID=abc\nmax_known_topic_id = -1\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                scraper.load_config(path)

    def test_update_max_known_topic_id(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scraper.cfg"
            path.write_text(
                "[scraper]\n"
                "cookie = PHPSESSID=abc\n"
                "max_known_topic_id = 10\n",
                encoding="utf-8",
            )
            config = scraper.load_config(path)
            scraper.update_max_known_topic_id(config, 42)
            updated = scraper.load_config(path)

            self.assertEqual(updated.max_known_topic_id, 42)

    def test_update_max_known_topic_id_preserves_default_section_configs(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scraper.cfg"
            path.write_text("[DEFAULT]\ncookie = PHPSESSID=abc\n", encoding="utf-8")
            config = scraper.load_config(path)
            scraper.update_max_known_topic_id(config, 42)
            updated = scraper.load_config(path)

            self.assertEqual(updated.cookie, "PHPSESSID=abc")
            self.assertEqual(updated.max_known_topic_id, 42)


class CrawlingTests(unittest.TestCase):
    def make_config(self, tmp):
        return scraper.ScraperConfig(
            config_path=Path(tmp) / "scraper.cfg",
            base_url="http://forum.redump.org",
            cookie="PHPSESSID=abc",
            max_known_topic_id=0,
            missing_stop_after=5,
            delay_seconds=0,
            workers=1,
            output_dir=tmp,
        )

    def make_config_with_missing_stop_after(self, tmp, missing_stop_after):
        return scraper.ScraperConfig(
            config_path=Path(tmp) / "scraper.cfg",
            base_url="http://forum.redump.org",
            cookie="PHPSESSID=abc",
            max_known_topic_id=0,
            missing_stop_after=missing_stop_after,
            delay_seconds=0,
            workers=1,
            output_dir=tmp,
        )

    def test_highest_completed_topic_id_ignores_zero_byte_markers(self):
        with tempfile.TemporaryDirectory() as tmp:
            topics_dir = Path(tmp) / "topics"
            topics_dir.mkdir()
            (topics_dir / "000003.json").write_text("{}", encoding="utf-8")
            (topics_dir / "000010.json").touch()
            (topics_dir / "000007.json").write_text("{}", encoding="utf-8")
            (topics_dir / "notes.json").write_text("{}", encoding="utf-8")

            self.assertEqual(scraper.highest_completed_topic_id(tmp), 7)

    def test_known_topic_high_water_uses_config_and_disk_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = self.make_config(tmp)
            object.__setattr__(config, "max_known_topic_id", 10)

            high_water = scraper.known_topic_high_water(
                config,
                highest_disk_topic_id=12,
            )

            self.assertEqual(high_water, 12)

    def test_process_topic_id_can_skip_missing_marker_for_discovery(self):
        original = scraper.scrape_topic_id

        def fake_scrape_topic_id(_session, _config, _topic_id, _forums_by_id):
            return None, True, []

        scraper.scrape_topic_id = fake_scrape_topic_id
        try:
            with tempfile.TemporaryDirectory() as tmp:
                config = self.make_config(tmp)
                topic_id, status, warnings, result_type = scraper.process_topic_id(
                    11,
                    config,
                    {},
                    write_missing_marker=False,
                )

                self.assertEqual(topic_id, 11)
                self.assertEqual(result_type, "missing")
                self.assertEqual(warnings, [])
                self.assertIn("not marked", status)
                self.assertFalse(Path(scraper.local_topic_path(tmp, 11)).exists())
        finally:
            scraper.scrape_topic_id = original

    def test_write_topic_json_strips_view_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = self.make_config(tmp)
            scraper.write_topic_json(
                {
                    "topic_id": 12,
                    "subject": "Example",
                    "view_count": 50,
                    "views": 51,
                    "topic_views": 52,
                },
                config,
            )
            written = Path(scraper.local_topic_path(tmp, 12)).read_text(encoding="utf-8")

            self.assertIn('"subject": "Example"', written)
            self.assertNotIn("view_count", written)
            self.assertNotIn("topic_views", written)
            self.assertNotIn('"views"', written)

    def test_process_topic_id_marks_historical_missing_by_default(self):
        original = scraper.scrape_topic_id

        def fake_scrape_topic_id(_session, _config, _topic_id, _forums_by_id):
            return None, True, []

        scraper.scrape_topic_id = fake_scrape_topic_id
        try:
            with tempfile.TemporaryDirectory() as tmp:
                config = self.make_config(tmp)
                scraper.process_topic_id(9, config, {})
                marker_path = Path(scraper.local_topic_path(tmp, 9))

                self.assertTrue(marker_path.exists())
                self.assertEqual(marker_path.stat().st_size, 0)
        finally:
            scraper.scrape_topic_id = original

    def test_discovery_marks_only_missing_ids_confirmed_by_later_topic(self):
        original = scraper.process_topic_id

        def fake_process_topic_id(
            topic_id,
            config,
            _forums_by_id,
            *,
            write_missing_marker=True,
            ignore_missing_marker=False,
        ):
            self.assertFalse(write_missing_marker)
            self.assertFalse(ignore_missing_marker)
            if topic_id == 2:
                return topic_id, "OK", [], "ok"
            return topic_id, "missing/inaccessible (not marked)", [], "missing"

        scraper.process_topic_id = fake_process_topic_id
        try:
            with tempfile.TemporaryDirectory() as tmp:
                config = self.make_config_with_missing_stop_after(tmp, 3)
                with contextlib.redirect_stdout(io.StringIO()):
                    max_confirmed_topic_id = scraper.run_discovery_phase(config, {}, 1)

                self.assertTrue(Path(scraper.local_topic_path(tmp, 1)).exists())
                self.assertFalse(Path(scraper.local_topic_path(tmp, 3)).exists())
                self.assertFalse(Path(scraper.local_topic_path(tmp, 4)).exists())
                self.assertFalse(Path(scraper.local_topic_path(tmp, 5)).exists())
                self.assertEqual(max_confirmed_topic_id, 2)
        finally:
            scraper.process_topic_id = original

    def test_discovery_retries_trailing_zero_byte_markers(self):
        original = scraper.process_topic_id
        calls = []

        def fake_process_topic_id(
            topic_id,
            config,
            _forums_by_id,
            *,
            write_missing_marker=True,
            ignore_missing_marker=False,
        ):
            calls.append((topic_id, write_missing_marker, ignore_missing_marker))
            if topic_id == 1:
                scraper.write_topic_json({"topic_id": 1}, config)
                return topic_id, "OK", [], "ok"
            return topic_id, "missing/inaccessible (not marked)", [], "missing"

        scraper.process_topic_id = fake_process_topic_id
        try:
            with tempfile.TemporaryDirectory() as tmp:
                config = self.make_config_with_missing_stop_after(tmp, 1)
                scraper.write_missing_topic_marker(tmp, 1)

                with contextlib.redirect_stdout(io.StringIO()):
                    max_confirmed_topic_id = scraper.run_discovery_phase(config, {}, 1)

                self.assertEqual(calls[0], (1, False, True))
                self.assertGreater(Path(scraper.local_topic_path(tmp, 1)).stat().st_size, 0)
                self.assertEqual(max_confirmed_topic_id, 1)
        finally:
            scraper.process_topic_id = original


if __name__ == "__main__":
    unittest.main()
