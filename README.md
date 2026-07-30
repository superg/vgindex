# vgindex - redump.info Website

vgindex is the source code behind [redump.info](https://redump.info/), a disc preservation database dedicated to collecting and sharing verified information about optical media.

The website was developed from scratch as a replacement for redump.org after the old site became unreliable under constant bot traffic and there was no practical way for the community to maintain or improve its aging codebase. Rather than carrying those limitations forward, vgindex provides a reliable, maintainable foundation shaped around modern optical disc preservation workflows and the long-term needs of the redump.info community.

## 🛠️ Technical Stack

- **Rust 2021**, **Axum**, and **Tokio**
- **Askama** server-rendered HTML templates
- **Pico CSS**, **HTMX**, and **Vanilla JavaScript**
- **PostgreSQL** with **SQLx**
- **Docker Compose** and **Caddy**

## 📄 License

This project is licensed under the GNU Affero General Public License v3.0 — see the [LICENSE](LICENSE) file for details.

## 👨‍💻 Author

**Hennadiy Brych** — [gennadiy.brych@gmail.com](mailto:gennadiy.brych@gmail.com)

---

**Need help?** [Open an issue](https://github.com/superg/vgindex/issues).
