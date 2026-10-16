import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("vendor-credseal.py")
spec = importlib.util.spec_from_file_location("vendor_credseal", SCRIPT)
vendor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vendor)


class VendorSnapshot(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="credseal-vendor-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / "source"
        self.destination = self.root / "vendor"
        self.source.mkdir()
        (self.source / "src").mkdir()
        (self.source / "Cargo.toml").write_text('[package]\nname = "credseal"\nversion = "0.1.0"\n')
        (self.source / "LICENSE").write_text("Apache-2.0\n")
        (self.source / "NOTICE").write_text("credseal\n")
        (self.source / "src/lib.rs").write_text("pub fn value() -> u8 { 7 }\n")
        patcher = patch.object(vendor, "DESTINATION", self.destination)
        patcher.start()
        self.addCleanup(patcher.stop)

    def run_command(self, *arguments):
        output = io.StringIO()
        with patch.object(sys, "argv", [str(SCRIPT), *map(str, arguments)]), contextlib.redirect_stdout(output):
            vendor.main()
        return output.getvalue()

    def test_sync_copies_source_and_records_a_verifiable_snapshot(self):
        (self.source / "src/.env").write_text("SECRET=do-not-copy\n")
        (self.source / "src/master.key").write_text("do-not-copy\n")
        self.assertEqual(self.run_command(self.source), "Synced credseal 0.1.0: 4 files\n")
        self.assertEqual((self.destination / "src/lib.rs").read_text(), "pub fn value() -> u8 { 7 }\n")
        self.assertFalse((self.destination / "src/.env").exists())
        self.assertFalse((self.destination / "src/master.key").exists())
        self.assertEqual(self.run_command("--verify"), "Verified 4 credseal source files\n")
        self.assertEqual(self.run_command("--check", self.source), "Snapshot matches credseal 0.1.0\n")

    def test_verification_ignores_local_build_and_secret_files(self):
        self.run_command(self.source)
        for name in ["target/debug/credseal", "src/.env.local", "tests/master.key", "tests/test.toml.enc.lock"]:
            path = self.destination / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("local output\n")
        self.assertEqual(self.run_command("--verify"), "Verified 4 credseal source files\n")
        self.assertEqual(self.run_command("--check", self.source), "Snapshot matches credseal 0.1.0\n")

    def test_sync_updates_changed_source_and_is_repeatable(self):
        self.run_command(self.source)
        (self.source / "src/lib.rs").write_text("pub fn value() -> u8 { 8 }\n")
        self.run_command(self.source)
        before = (self.destination / "SOURCE.json").read_bytes()
        self.run_command(self.source)
        self.assertEqual((self.destination / "src/lib.rs").read_text(), "pub fn value() -> u8 { 8 }\n")
        self.assertEqual((self.destination / "SOURCE.json").read_bytes(), before)

    def test_a_modified_vendor_file_is_not_overwritten(self):
        self.run_command(self.source)
        path = self.destination / "src/lib.rs"
        path.write_text("a local change\n")
        with self.assertRaisesRegex(SystemExit, "Vendored file differs from SOURCE.json: src/lib.rs"):
            self.run_command(self.source)
        self.assertEqual(path.read_text(), "a local change\n")

    def test_verification_does_not_need_the_source_checkout(self):
        self.run_command(self.source)
        self.source.rename(self.root / "source-not-here")
        self.assertEqual(self.run_command("--verify"), "Verified 4 credseal source files\n")

    def test_check_refuses_a_stale_snapshot_without_writing(self):
        self.run_command(self.source)
        (self.source / "src/lib.rs").write_text("pub fn value() -> u8 { 8 }\n")
        with self.assertRaisesRegex(SystemExit, "source differs from the vendored snapshot"):
            self.run_command("--check", self.source)
        self.assertEqual((self.destination / "src/lib.rs").read_text(), "pub fn value() -> u8 { 7 }\n")

    def test_a_source_link_is_not_followed(self):
        path = self.source / "src/lib.rs"
        path.unlink()
        outside = self.root / "private"
        outside.write_text("do-not-copy\n")
        path.symlink_to(outside)
        with self.assertRaisesRegex(SystemExit, "Source symbolic links are not supported: src/lib.rs"):
            self.run_command(self.source)
        self.assertFalse(self.destination.exists())

    def test_an_unrecorded_vendor_link_is_not_followed(self):
        self.run_command(self.source)
        (self.destination / "outside").symlink_to(self.source, target_is_directory=True)
        with self.assertRaisesRegex(SystemExit, "Vendor symbolic links are not supported"):
            self.run_command(self.source)
        self.assertEqual((self.source / "src/lib.rs").read_text(), "pub fn value() -> u8 { 7 }\n")

    def test_a_record_cannot_name_a_path_outside_the_snapshot(self):
        self.run_command(self.source)
        record = self.destination / "SOURCE.json"
        data = json.loads(record.read_text())
        data["files"]["../private"] = "0" * 64
        record.write_text(json.dumps(data))
        with self.assertRaisesRegex(SystemExit, "Invalid vendor/credseal/SOURCE.json"):
            self.run_command("--verify")


if __name__ == "__main__":
    unittest.main()
