import json
import tempfile
import unittest
from pathlib import Path

from scripts.check_deployment_metadata import validate


def contract_id(character):
    return "C" + character * 55


class DeploymentMetadataTests(unittest.TestCase):
    def setUp(self):
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.addresses = {
            "registry": contract_id("A"),
            "invoice": contract_id("B"),
            "escrow_usdc": contract_id("C"),
            "pool_usdc": contract_id("D"),
        }
        self._write_readme()

    def tearDown(self):
        self.tempdir.cleanup()

    def _write_readme(self, markers=True):
        rows = "\n".join(
            f"| {name} | `{self.addresses[key]}` |"
            for key, name in (
                ("registry", "registry_contract"),
                ("invoice", "invoice_contract"),
                ("escrow_usdc", "escrow_contract"),
                ("pool_usdc", "pool_contract"),
            )
        )
        marker_text = (
            "<!-- START_DEPLOYED_ADDRESSES -->\n"
            f"{rows}\n"
            "<!-- END_DEPLOYED_ADDRESSES -->"
            if markers
            else rows
        )
        (self.root / "README.md").write_text(marker_text)

    def test_readme_without_local_generated_files_is_valid(self):
        self.assertEqual(validate(self.root), [])

    def test_generated_files_matching_readme_are_valid(self):
        (self.root / "deployments.json").write_text(json.dumps(self.addresses))
        env_names = {
            "registry": "NEXT_PUBLIC_REGISTRY_CONTRACT_ID",
            "invoice": "NEXT_PUBLIC_INVOICE_CONTRACT_ID",
            "escrow_usdc": "NEXT_PUBLIC_ESCROW_USDC_CONTRACT_ID",
            "pool_usdc": "NEXT_PUBLIC_POOL_USDC_CONTRACT_ID",
        }
        (self.root / ".env.deployed").write_text(
            "\n".join(
                f"{env_names[key]}={value}" for key, value in self.addresses.items()
            )
        )
        (self.root / ".deployed-addresses").write_text(
            "\n".join(f"{key}={value}" for key, value in self.addresses.items())
        )
        self.assertEqual(validate(self.root), [])

    def test_missing_markers_fail_clearly(self):
        self._write_readme(markers=False)
        errors = validate(self.root)
        self.assertTrue(any("address marker" in error for error in errors))

    def test_stale_generated_address_fails(self):
        stale = dict(self.addresses, pool_usdc=contract_id("E"))
        (self.root / "deployments.json").write_text(json.dumps(stale))
        errors = validate(self.root)
        self.assertTrue(any("stale" in error for error in errors))

    def test_generated_files_disagreeing_fails(self):
        (self.root / "deployments.json").write_text(json.dumps(self.addresses))
        (self.root / ".env.deployed").write_text(
            "NEXT_PUBLIC_REGISTRY_CONTRACT_ID=" + contract_id("E")
        )
        errors = validate(self.root)
        self.assertTrue(any("contract IDs disagree" in error for error in errors))

    def test_experimental_inline_comment_is_ignored(self):
        extended = dict(
            self.addresses,
            escrow_xlm=contract_id("F"),
            pool_xlm=contract_id("G"),
        )
        (self.root / "deployments.json").write_text(json.dumps(extended))
        (self.root / ".env.deployed").write_text(
            "\n".join(
                (
                    f"{env_key}={extended[address_key]}"
                    + (" # EXPERIMENTAL" if address_key.endswith("_xlm") else "")
                )
                for env_key, address_key in {
                    "NEXT_PUBLIC_REGISTRY_CONTRACT_ID": "registry",
                    "NEXT_PUBLIC_INVOICE_CONTRACT_ID": "invoice",
                    "NEXT_PUBLIC_ESCROW_USDC_CONTRACT_ID": "escrow_usdc",
                    "NEXT_PUBLIC_ESCROW_XLM_CONTRACT_ID": "escrow_xlm",
                    "NEXT_PUBLIC_POOL_USDC_CONTRACT_ID": "pool_usdc",
                    "NEXT_PUBLIC_POOL_XLM_CONTRACT_ID": "pool_xlm",
                }.items()
            )
        )
        self.assertEqual(validate(self.root), [])

    def test_invalid_contract_id_fails_address_boundary(self):
        invalid = dict(self.addresses, registry="C" + "A" * 54)
        (self.root / "deployments.json").write_text(json.dumps(invalid))
        errors = validate(self.root)
        self.assertTrue(any("valid contract address" in error for error in errors))


if __name__ == "__main__":
    unittest.main()