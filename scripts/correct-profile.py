#!/usr/bin/env python3
"""Apply an explicit, hash-bound JSON correction manifest."""

import argparse
import copy
import hashlib
import json
from pathlib import Path


SUBMISSION_PROFILE_SCHEMA_VERSION = 7
SUBMISSION_ALLOWED_POINTERS = frozenset(
    {
        "/profile_kind",
        "/run_complete",
        "/base_node/tip_end_height",
        "/base_node/tip_end_hash",
        "/base_node/authority_tip_end_height",
        "/base_node/authority_tip_end_hash",
        "/provenance/correction",
    }
)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def set_pointer(document: object, pointer: str, value: object) -> None:
    if not pointer.startswith("/"):
        raise ValueError(f"invalid JSON Pointer: {pointer}")
    parts = [part.replace("~1", "/").replace("~0", "~") for part in pointer[1:].split("/")]
    if not parts:
        raise ValueError("root replacement is not supported")
    target = document
    for part in parts[:-1]:
        target = target[int(part)] if isinstance(target, list) else target[part]
    leaf = parts[-1]
    if isinstance(target, list):
        target[int(leaf)] = value
    else:
        target[leaf] = value


def validate_manifest(document: object, manifest: object) -> None:
    if not isinstance(document, dict) or not isinstance(manifest, dict):
        raise ValueError("profile and correction manifest must be JSON objects")
    schema_version = document.get("schema_version")
    purpose = manifest.get("profile_purpose")
    if purpose == "submission":
        if not isinstance(schema_version, int) or schema_version < SUBMISSION_PROFILE_SCHEMA_VERSION:
            raise ValueError(
                f"submission corrections require schema version {SUBMISSION_PROFILE_SCHEMA_VERSION} or newer"
            )
        transformations = manifest.get("transformations")
        if not isinstance(transformations, list) or not transformations:
            raise ValueError("submission correction manifest must contain transformations")
        pointers = [item.get("pointer") for item in transformations if isinstance(item, dict)]
        if len(pointers) != len(transformations) or any(not isinstance(pointer, str) for pointer in pointers):
            raise ValueError("every correction transformation must contain a string pointer")
        if len(set(pointers)) != len(pointers):
            raise ValueError("correction manifest contains duplicate pointers")
        rejected = sorted(set(pointers).difference(SUBMISSION_ALLOWED_POINTERS))
        if rejected:
            raise ValueError(
                "submission correction may change only finalization metadata; rejected pointers: "
                + ", ".join(rejected)
            )
    elif purpose == "historical_non_submission":
        if not isinstance(schema_version, int) or schema_version >= SUBMISSION_PROFILE_SCHEMA_VERSION:
            raise ValueError(
                "schema-v7 and newer profiles cannot opt out of submission correction restrictions"
            )
    else:
        raise ValueError(
            "correction manifest profile_purpose must be 'submission' or 'historical_non_submission'"
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--raw", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    raw_bytes = args.raw.read_bytes()
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    actual = digest(raw_bytes)
    if actual != manifest["raw_profile_sha256"]:
        raise SystemExit(f"raw profile SHA-256 mismatch: expected {manifest['raw_profile_sha256']}, got {actual}")
    evidence = manifest.get("evidence")
    if evidence is not None:
        evidence_path = Path(evidence["path"])
        evidence_actual = digest(evidence_path.read_bytes())
        if evidence_actual != evidence["sha256"]:
            raise SystemExit(
                f"evidence SHA-256 mismatch: expected {evidence['sha256']}, got {evidence_actual}"
            )
    document = json.loads(raw_bytes)
    try:
        validate_manifest(document, manifest)
    except ValueError as error:
        raise SystemExit(f"invalid correction manifest: {error}") from error
    for transformation in manifest["transformations"]:
        set_pointer(document, transformation["pointer"], copy.deepcopy(transformation["value"]))
    output = (json.dumps(document, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode("utf-8")
    args.out.write_bytes(output)
    print(json.dumps({
        "raw_profile_sha256": actual,
        "corrected_profile_sha256": digest(output),
        "transformations": [item["pointer"] for item in manifest["transformations"]],
    }, sort_keys=True))


if __name__ == "__main__":
    main()
