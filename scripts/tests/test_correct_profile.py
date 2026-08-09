import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "correct-profile.py"
SPEC = importlib.util.spec_from_file_location("correct_profile", SCRIPT)
CORRECT_PROFILE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(CORRECT_PROFILE)


class CorrectionPolicyTests(unittest.TestCase):
    def test_submission_accepts_finalization_metadata(self):
        document = {"schema_version": 7}
        manifest = {
            "profile_purpose": "submission",
            "transformations": [
                {"pointer": "/profile_kind", "value": "final"},
                {"pointer": "/run_complete", "value": True},
            ],
        }
        CORRECT_PROFILE.validate_manifest(document, manifest)

    def test_submission_rejects_measured_cell_pointer(self):
        document = {"schema_version": 7}
        manifest = {
            "profile_purpose": "submission",
            "transformations": [
                {
                    "pointer": "/modes/new_wallet/scenarios/S1/repetitions/0/metrics/outcome_counts",
                    "value": {},
                }
            ],
        }
        with self.assertRaisesRegex(ValueError, "rejected pointers"):
            CORRECT_PROFILE.validate_manifest(document, manifest)

    def test_current_schema_cannot_claim_historical_policy(self):
        document = {"schema_version": 7}
        manifest = {
            "profile_purpose": "historical_non_submission",
            "transformations": [{"pointer": "/profile_kind", "value": "final"}],
        }
        with self.assertRaisesRegex(ValueError, "cannot opt out"):
            CORRECT_PROFILE.validate_manifest(document, manifest)

    def test_legacy_evidence_can_be_reconstructed_explicitly(self):
        document = {"schema_version": 6}
        manifest = {
            "profile_purpose": "historical_non_submission",
            "transformations": [
                {
                    "pointer": "/modes/old_wallet/scenarios/S1/repetitions/0/metrics/outcome_counts",
                    "value": {},
                }
            ],
        }
        CORRECT_PROFILE.validate_manifest(document, manifest)


if __name__ == "__main__":
    unittest.main()
