"""Offline tests for audit coverage/finite guards; no model, network or NumPy."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "audit_codehygiene", Path(__file__).with_name("audit-codehygiene.py")
)
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class AuditGuards(unittest.TestCase):
    def test_windows_cover_every_line(self):
        for count in (0, 1, 63, 64, 255, 256, 257, 448, 449, 2048, 50000):
            with self.subTest(count=count):
                chunks = list(audit.windows(count, 256, 64))
                covered = set()
                for start, end in chunks:
                    self.assertGreaterEqual(start, 0)
                    self.assertLessEqual(end, count)
                    self.assertLessEqual(end - start, 256)
                    covered.update(range(start, end))
                self.assertEqual(covered, set(range(count)))
                self.assertEqual(chunks[-1][1], count)

    def test_overlap_and_empty_contract(self):
        self.assertEqual(list(audit.windows(0, 256, 64)), [(0, 0)])
        self.assertEqual(list(audit.windows(449, 256, 64)),
                         [(0, 256), (192, 448), (384, 449)])
        self.assertEqual(list(audit.windows(129, 64, 0)),
                         [(0, 64), (64, 128), (128, 129)])

    def test_accepts_finite_nested_output(self):
        audit.require_finite({"score": 0.5, "windows": [{"prob": 1.0}],
                              "extra": (None, True, "name", 2, -0.0)})

    def test_rejects_nonfinite_nested_output(self):
        for value in (float("nan"), float("inf"), -float("inf")):
            with self.subTest(value=value), self.assertRaises(ValueError):
                audit.require_finite({"windows": [(0, {"prob": value})]})

    def test_digest_is_sha256(self):
        self.assertEqual(audit.digest(b""),
                         "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")


if __name__ == "__main__":
    unittest.main()
