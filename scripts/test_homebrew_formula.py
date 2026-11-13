import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from homebrew_formula import ensure_monotonic, render


class FormulaTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.assets = Path(self.temp.name)
        for arch in ('arm64', 'x86_64'):
            path = self.assets / f'wired-magic-v0.1.0-darwin-{arch}.tar.gz'
            with tarfile.open(path, 'w:gz') as archive:
                member = tarfile.TarInfo('wm')
                member.mode = 0o755
                body = f'test fixture {arch}'.encode()
                member.size = len(body)
                archive.addfile(member, io.BytesIO(body))
        self.write_sums(('arm64', 'x86_64'))

    def write_sums(self, archs):
        sums = []
        for arch in archs:
            path = self.assets / f'wired-magic-v0.1.0-darwin-{arch}.tar.gz'
            sums.append(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n')
        (self.assets / 'SHA256SUMS').write_text(''.join(sums))

    def generate(self, repo='CaioFaSoares/wired-magic-tools', tag='v0.1.0'):
        return render(repo, tag, self.assets)

    def test_formula_names_urls_and_real_hashes(self):
        formula = self.generate()
        self.assertIn('class WiredMagic < Formula', formula)
        self.assertIn('bin.install "wm"', formula)
        self.assertIn('version "0.1.0"', formula)
        for line in (self.assets / 'SHA256SUMS').read_text().splitlines():
            digest, name = line.split()
            self.assertIn(f'sha256 "{digest}"', formula)
            self.assertIn('/releases/download/v0.1.0/' + name, formula)

    def test_monotonic_first_install_same_and_numeric_upgrade(self):
        existing = self.assets / 'wired-magic.rb'
        candidate = '  version "1.10.0"\n'
        ensure_monotonic(existing, candidate)
        for old in ('1.9.0', '1.10.0', '0.99.99'):
            existing.write_text(f'  version "{old}"\n')
            ensure_monotonic(existing, candidate)

    def test_monotonic_rejects_downgrade_and_unknown_existing(self):
        existing = self.assets / 'wired-magic.rb'
        for text in ('version "1.10.0"', 'version "2.0.0"', 'version "1.0.0-rc.1"', '', 'version "1.0.0"\nversion "2.0.0"'):
            with self.subTest(text=text):
                existing.write_text(text)
                with self.assertRaises(ValueError):
                    ensure_monotonic(existing, 'version "1.9.0"\n')

    def test_reject_invalid_tags(self):
        for tag in ('0.1.0', 'v01.1.0', 'v1.0.0-rc.1', 'v1.0.0\n', 'v1.0.0"'):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                self.generate(tag=tag)

    def test_reject_repository_injection(self):
        with self.assertRaises(ValueError):
            self.generate(repo='owner/repo"')

    def test_reject_changed_checksum(self):
        (self.assets / 'SHA256SUMS').write_text('')
        with self.assertRaises(ValueError):
            self.generate()

    def test_single_arm_formula_refuses_intel(self):
        self.write_sums(('arm64',))
        formula = render('CaioFaSoares/wired-magic-tools', 'v0.1.0', self.assets, ('arm64',))
        self.assertIn('depends_on arch: :arm64', formula)
        self.assertIn('on_arm do', formula)
        self.assertNotIn('on_intel', formula)
        self.assertNotIn('darwin-x86_64', formula)

    def test_single_intel_formula_refuses_arm(self):
        self.write_sums(('x86_64',))
        formula = render('CaioFaSoares/wired-magic-tools', 'v0.1.0', self.assets, ('x86_64',))
        self.assertIn('depends_on arch: :x86_64', formula)
        self.assertIn('on_intel do', formula)
        self.assertNotIn('on_arm', formula)
        self.assertNotIn('darwin-arm64', formula)

    def test_reject_invalid_or_unmatched_arch_subsets(self):
        for archs in ((), ('amd64',), ('arm64', 'arm64')):
            with self.subTest(archs=archs), self.assertRaises(ValueError):
                render('owner/repo', 'v0.1.0', self.assets, archs)
        with self.assertRaises(ValueError):
            render('owner/repo', 'v0.1.0', self.assets, ('arm64',))

    def test_reject_missing_architecture(self):
        (self.assets / 'wired-magic-v0.1.0-darwin-x86_64.tar.gz').rename(self.assets / 'missing')
        with self.assertRaises(FileNotFoundError):
            self.generate()

    def test_reject_unsafe_archive(self):
        path = self.assets / 'wired-magic-v0.1.0-darwin-arm64.tar.gz'
        with tarfile.open(path, 'w:gz') as archive:
            member = tarfile.TarInfo('../wm')
            member.mode = 0o755
            archive.addfile(member)
        with self.assertRaises(ValueError):
            self.generate()


if __name__ == '__main__':
    unittest.main()
