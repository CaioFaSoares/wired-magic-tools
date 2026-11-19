#!/usr/bin/env python3
"""Offline tests: packaging does not invoke Node, Rust, or a live server."""
import hashlib
import json
import re
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import package_control_panel as panel


class PanelPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.dist = self.root / "dist"
        (self.dist / "assets").mkdir(parents=True)
        (self.dist / "index.html").write_bytes(b"<html><script src='/assets/app.js'></script></html>")
        (self.dist / "assets/app.js").write_bytes(b"console.log('trusted package');")
        self.metadata = b'id = "control_panel"\nversion = "test"\nnode_driver = "none"\n'

    def test_manifest_hashes_every_allowlisted_file(self):
        files, raw = panel.collect_assets(self.dist, "test")
        manifest = json.loads(raw)
        self.assertEqual(manifest["schema"], 1)
        self.assertEqual(manifest["id"], "control_panel")
        self.assertEqual(manifest["files"], {name: hashlib.sha256(data).hexdigest() for name, data in files.items()})
        self.assertEqual(set(files), {"index.html", "assets/app.js"})

    def test_archive_is_deterministic_bounded_regular_files_only_and_never_activates(self):
        first, second = self.root / "first.tar.gz", self.root / "second.tar.gz"
        checksum, digest = panel.package(self.dist, first, "test", self.metadata)
        self.assertEqual(panel.package(self.dist, second, "test", self.metadata), (checksum, digest))
        self.assertEqual(first.read_bytes(), second.read_bytes())
        self.assertEqual(hashlib.sha256(first.read_bytes()).hexdigest(), checksum)
        self.assertEqual(first.with_name(first.name + ".sha256").read_text(), f"{checksum}  first.tar.gz\n")
        with tarfile.open(first) as archive:
            entries = archive.getmembers()
            self.assertEqual({entry.name for entry in entries}, {
                "control_panel/extension.toml", "control_panel/www/bundle.json",
                "control_panel/www/index.html", "control_panel/www/assets/app.js"})
            self.assertTrue(all(entry.isfile() and entry.mode == 0o644 and entry.mtime == 0 for entry in entries))
            raw = archive.extractfile("control_panel/www/bundle.json").read()
            self.assertEqual(hashlib.sha256(raw).hexdigest(), digest)
        self.assertFalse((self.root / "control-panel-trust.sha256").exists())

    def test_never_overwrites_existing_package_or_checksum(self):
        output = self.root / "output.tar.gz"
        output.write_bytes(b"keep")
        with self.assertRaises(FileExistsError):
            panel.package(self.dist, output, "test", self.metadata)
        self.assertEqual(output.read_bytes(), b"keep")
        output2 = self.root / "output2.tar.gz"
        output2.with_name(output2.name + ".sha256").write_text("keep")
        with self.assertRaises(FileExistsError):
            panel.package(self.dist, output2, "test", self.metadata)
        self.assertFalse(output2.exists())

    def test_missing_index_is_rejected(self):
        (self.dist / "index.html").unlink()
        with self.assertRaisesRegex(ValueError, "index.html"):
            panel.collect_assets(self.dist, "test")

    def test_symlink_file_directory_and_dist_are_rejected(self):
        outside = self.root / "outside.js"
        outside.write_bytes(b"must not package")
        link = self.dist / "assets/leak.js"
        link.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "regular file"):
            panel.collect_assets(self.dist, "test")
        link.unlink()
        (self.dist / "linked").symlink_to(self.root, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink directory"):
            panel.collect_assets(self.dist, "test")
        root_link = self.root / "dist-link"
        root_link.symlink_to(self.dist, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "real directory"):
            panel.collect_assets(root_link, "test")

    def test_extra_secrets_sourcemaps_and_html_are_rejected(self):
        for name in [".env", "config.json", "app.js.map", "other.html", "bundle.json", "space file.js"]:
            path = self.dist / name
            path.write_bytes(b"not public")
            with self.assertRaisesRegex(ValueError, "unsupported asset"):
                panel.collect_assets(self.dist, "test")
            path.unlink()

    def test_name_validator_rejects_traversal_encoding_and_non_ascii(self):
        for name in ["../x.js", "./x.js", "/x.js", "a//x.js", "a/../x.js", "%2e%2e/x.js", "x\\y.js", "à.js", "a/", "a" * 241]:
            self.assertFalse(panel.safe_name(name), name)
        for name in ["index.html", "assets/vendor-123.js", "fonts/font_abc.woff2"]:
            self.assertTrue(panel.safe_name(name), name)

    def test_file_total_count_and_manifest_limits(self):
        for key, limit in [("MAX_FILE_BYTES", 1), ("MAX_TOTAL_BYTES", 1), ("MAX_FILES", 1), ("MAX_MANIFEST_BYTES", 1)]:
            with patch.object(panel, key, limit):
                with self.assertRaises(ValueError, msg=key):
                    panel.collect_assets(self.dist, "test")

    def test_react_pages_do_not_shadow_reserved_server_namespaces(self):
        app = (panel.ROOT / "web/src/App.tsx").read_text()
        routes = re.findall(r'<Route path="([^"]+)"', app)
        self.assertIn("/relay-status", routes)
        for route in routes:
            for namespace in ["/api", "/relay", "/healthz", "/j", "/download"]:
                self.assertFalse(route == namespace or route.startswith(namespace + "/"), route)

    def test_default_server_builds_have_no_frontend_prerequisite(self):
        docker = (panel.ROOT / "Dockerfile").read_text()
        self.assertNotIn("FROM node:", docker)
        self.assertNotIn("web/dist ./", docker)
        self.assertNotIn("cargo clean", docker)
        control = (panel.ROOT / "crates/wm-control/Cargo.toml").read_text()
        self.assertNotIn("rust-embed", control)
        self.assertNotIn("mime_guess", control)
        release = (panel.ROOT / "scripts/publish_macos.sh").read_text()
        self.assertNotIn("pnpm --dir web", release)
        workflow = (panel.ROOT / ".github/workflows/release-macos.yml").read_text()
        self.assertNotIn("actions/setup-node", workflow)
        self.assertNotIn("pnpm/action-setup", workflow)


if __name__ == "__main__":
    unittest.main()
