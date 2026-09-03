import json
import tempfile
import unittest
from pathlib import Path

from run_harness import layouts_for_bundler, split_evidence


class HarnessVerificationTests(unittest.TestCase):
    def write_provenance(self, modules):
        temp_dir = tempfile.TemporaryDirectory()
        unpacked = Path(temp_dir.name)
        (unpacked / "provenance.json").write_text(json.dumps({"modules": modules}))
        self.addCleanup(temp_dir.cleanup)
        return unpacked

    def test_multiple_passthrough_inputs_do_not_count_as_a_split(self):
        unpacked = self.write_provenance({
            "a.js": {"input": "/dist/a.js"},
            "b.js": {"input": "/dist/b.js"},
        })

        did_split, metrics = split_evidence(unpacked)

        self.assertFalse(did_split)
        self.assertEqual(metrics["outputs"], 2)
        self.assertEqual(metrics["inputs"], 2)
        self.assertEqual(metrics["expanded_inputs"], 0)

    def test_one_input_expanding_to_multiple_modules_counts_as_a_split(self):
        unpacked = self.write_provenance({
            "entry.js": {"input": "/dist/bundle.js"},
            "module-1.js": {"input": "/dist/bundle.js"},
        })

        did_split, metrics = split_evidence(unpacked)

        self.assertTrue(did_split)
        self.assertEqual(metrics["outputs"], 2)
        self.assertEqual(metrics["inputs"], 1)
        self.assertEqual(metrics["expanded_inputs"], 1)

    def test_iife_and_split_are_explicit_esbuild_and_rollup_variants(self):
        requested = ["iife", "split"]

        self.assertEqual(layouts_for_bundler("esbuild", requested), requested)
        self.assertEqual(layouts_for_bundler("rollup", requested), requested)
        self.assertEqual(layouts_for_bundler("webpack5", requested), [None])


if __name__ == "__main__":
    unittest.main()
