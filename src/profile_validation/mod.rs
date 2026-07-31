use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, bail};
use serde_json::{Value, json};

use super::{
    CellStatus, LEGACY_RESULT_SCHEMA_VERSION, OutcomeCounts, ProfileKind,
    REFERENCE_BASE_NODE_REVISION, RESULT_SCHEMA_VERSION, ResultProfile,
};
use crate::{
    modes::{ModeName, ScenarioName},
    versions::TX_MINED_CONFIRMED_STATUS,
};

const REQUIRED_STAGES: [&str; 4] = [
    "old_wallet",
    "new_wallet",
    "payment_processor",
    "fresh_scans",
];
const REFERENCE_S1_ROUNDS: [(u64, u64, u64, u64); 7] = [
    (1, 1, 2, 2),
    (2, 2, 2, 4),
    (3, 4, 2, 8),
    (4, 8, 2, 16),
    (5, 16, 2, 32),
    (6, 32, 2, 64),
    (7, 64, 8, 512),
];
const REFERENCE_S4_RAMP: [u64; 5] = [8, 16, 32, 64, 128];

pub fn schema_document() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://github.com/tari-project/wallet-benchmarks/blob/main/RESULT_PROFILE_SCHEMA.json",
        "title": "Tari wallet benchmark result profile",
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version", "run_id", "profile_kind", "run_complete",
            "provenance", "completed_stages", "generated_at", "network",
            "base_node", "environment", "versions", "config", "funding", "modes",
            "computed_deltas", "findings", "chain_verification"
        ],
        "properties": {
            "schema_version": {"const": RESULT_SCHEMA_VERSION},
            "run_id": {"type": "string", "minLength": 1},
            "profile_kind": {"enum": ["checkpoint", "final"]},
            "run_complete": {"type": "boolean"},
            "provenance": {"$ref": "#/$defs/provenance"},
            "completed_stages": {
                "type": "array", "items": {"type": "string", "minLength": 1},
                "uniqueItems": true
            },
            "generated_at": {"type": "string", "format": "date-time"},
            "network": {"type": "string", "minLength": 1},
            "base_node": {"$ref": "#/$defs/base_node"},
            "environment": {"$ref": "#/$defs/environment"},
            "versions": {
                "type": "object", "additionalProperties": {"type": "string"}
            },
            "config": {"$ref": "#/$defs/config"},
            "funding": {"$ref": "#/$defs/funding"},
            "modes": {"$ref": "#/$defs/modes"},
            "computed_deltas": {"$ref": "#/$defs/computed_deltas"},
            "findings": {"type": "array", "items": {"$ref": "#/$defs/finding"}},
            "chain_verification": {"$ref": "#/$defs/chain_verification"}
        },
        "$defs": {
            "nullable_string": {"type": ["string", "null"]},
            "nullable_integer": {"type": ["integer", "null"], "minimum": 0},
            "provenance": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "measurement_commit", "export_commit",
                    "measurement_worktree_clean", "export_worktree_clean",
                    "measurement_build_worktree_clean", "export_build_worktree_clean",
                    "measurement_build_manifest", "export_build_manifest"
                ],
                "properties": {
                    "measurement_commit": {"type": "string", "minLength": 1},
                    "export_commit": {"type": "string", "minLength": 1},
                    "measurement_worktree_clean": {"type": "boolean"},
                    "export_worktree_clean": {"type": "boolean"},
                    "measurement_build_worktree_clean": {"type": "boolean"},
                    "export_build_worktree_clean": {"type": "boolean"},
                    "measurement_build_manifest": {"anyOf": [{"$ref": "#/$defs/build_manifest"}, {"type": "null"}]},
                    "export_build_manifest": {"anyOf": [{"$ref": "#/$defs/build_manifest"}, {"type": "null"}]},
                    "correction": {"$ref": "#/$defs/profile_correction"}
                }
            },
            "profile_correction": {
                "type": "object", "additionalProperties": false,
                "required": ["manifest_schema_version", "manifest_path", "tool", "tool_version", "corrected_at", "raw_profile_sha256", "raw_profile_size", "transformations"],
                "properties": {
                    "manifest_schema_version": {"type": "integer", "minimum": 1},
                    "manifest_path": {"type": "string", "minLength": 1},
                    "tool": {"type": "string", "minLength": 1},
                    "tool_version": {"type": "string", "minLength": 1},
                    "corrected_at": {"type": "string", "format": "date-time"},
                    "raw_profile_sha256": {"type": "string", "pattern": "^[0-9a-fA-F]{64}$"},
                    "raw_profile_size": {"type": "integer", "minimum": 1},
                    "transformations": {"type": "array", "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["pointer", "value"],
                        "properties": {
                            "pointer": {"type": "string", "minLength": 1, "pattern": "^/"},
                            "value": {}
                        }
                    }}
                }
            },
            "build_manifest": {
                "type": "object", "additionalProperties": false,
                "required": ["schema_version", "sources", "artifacts"],
                "properties": {
                    "schema_version": {"const": 2},
                    "sources": {
                        "type": "object", "additionalProperties": false,
                        "required": ["minotari_cli", "tari_console_wallet", "minotari_node", "payment_processor"],
                        "properties": {
                            "minotari_cli": {"$ref": "#/$defs/build_source_one_patch"},
                            "tari_console_wallet": {"$ref": "#/$defs/build_source_no_patches"},
                            "minotari_node": {"$ref": "#/$defs/build_source_no_patches"},
                            "payment_processor": {"$ref": "#/$defs/build_source_one_patch"}
                        }
                    },
                    "artifacts": {
                        "type": "object", "additionalProperties": false,
                        "required": ["minotari", "minotari_console_wallet", "minotari_node", "minotari_payment_processor"],
                        "properties": {
                            "minotari": {"$ref": "#/$defs/build_artifact"},
                            "minotari_console_wallet": {"$ref": "#/$defs/build_artifact"},
                            "minotari_node": {"$ref": "#/$defs/build_artifact"},
                            "minotari_payment_processor": {"$ref": "#/$defs/build_artifact"}
                        }
                    }
                }
            },
            "build_source": {
                "type": "object", "additionalProperties": false,
                "required": ["repository", "upstream", "patches", "complete_diff_sha256", "result_tree"],
                "properties": {
                    "repository": {"type": "string", "minLength": 1},
                    "upstream": {"$ref": "#/$defs/build_upstream"},
                    "patches": {"type": "array", "items": {"$ref": "#/$defs/build_patch"}},
                    "complete_diff_sha256": {"$ref": "#/$defs/lowercase_sha256"},
                    "result_tree": {"$ref": "#/$defs/git_object_id"}
                }
            },
            "build_source_one_patch": {
                "allOf": [
                    {"$ref": "#/$defs/build_source"},
                    {"properties": {"patches": {"minItems": 1, "maxItems": 1}}}
                ]
            },
            "build_source_no_patches": {
                "allOf": [
                    {"$ref": "#/$defs/build_source"},
                    {"properties": {"patches": {"maxItems": 0}}}
                ]
            },
            "build_upstream": {
                "type": "object", "additionalProperties": false,
                "required": ["revision", "commit", "tree"],
                "properties": {
                    "revision": {"type": "string", "minLength": 1},
                    "commit": {"$ref": "#/$defs/git_object_id"},
                    "tree": {"$ref": "#/$defs/git_object_id"}
                }
            },
            "build_patch": {
                "type": "object", "additionalProperties": false,
                "required": ["path", "sha256", "result_tree"],
                "properties": {
                    "path": {"type": "string", "minLength": 1},
                    "sha256": {"$ref": "#/$defs/lowercase_sha256"},
                    "result_tree": {"$ref": "#/$defs/git_object_id"}
                }
            },
            "build_artifact": {
                "type": "object", "additionalProperties": false,
                "required": ["source", "source_revision", "source_tree", "sha256"],
                "properties": {
                    "source": {"type": "string", "minLength": 1},
                    "source_revision": {"type": "string", "minLength": 1},
                    "source_tree": {"$ref": "#/$defs/git_object_id"},
                    "sha256": {"$ref": "#/$defs/lowercase_sha256"}
                }
            },
            "git_object_id": {"type": "string", "pattern": "^[0-9a-f]{40}$"},
            "lowercase_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
            "base_node": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "endpoint", "authority_endpoint", "configured_revision", "observed_version", "version_observable",
                    "tip_start_height", "tip_start_hash", "tip_end_height", "tip_end_hash",
                    "pruning_horizon", "pruned_height", "is_synced", "authority_tip_start_height",
                    "authority_tip_start_hash", "authority_tip_end_height", "authority_tip_end_hash"
                ],
                "properties": {
                    "endpoint": {"type": "string", "minLength": 1},
                    "authority_endpoint": {"type": "string", "minLength": 1},
                    "configured_revision": {"type": "string", "minLength": 1},
                    "observed_version": {"$ref": "#/$defs/nullable_string"},
                    "version_observable": {"type": "boolean"},
                    "tip_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_start_hash": {"$ref": "#/$defs/nullable_string"},
                    "tip_end_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_end_hash": {"$ref": "#/$defs/nullable_string"},
                    "pruning_horizon": {"$ref": "#/$defs/nullable_integer"},
                    "pruned_height": {"$ref": "#/$defs/nullable_integer"},
                    "is_synced": {"type": ["boolean", "null"]},
                    "authority_tip_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "authority_tip_start_hash": {"$ref": "#/$defs/nullable_string"},
                    "authority_tip_end_height": {"$ref": "#/$defs/nullable_integer"},
                    "authority_tip_end_hash": {"$ref": "#/$defs/nullable_string"}
                }
            },
            "environment": {
                "type": "object", "additionalProperties": false,
                "required": ["os", "cpu_brand", "physical_cores", "total_memory_bytes", "base_node_network_path", "authority_network_path"],
                "properties": {
                    "os": {"type": "string"},
                    "cpu_brand": {"type": "string"},
                    "physical_cores": {"$ref": "#/$defs/nullable_integer"},
                    "total_memory_bytes": {"type": "integer", "minimum": 0},
                    "disk_kind": {"type": "string"},
                    "disk_name": {"type": "string"},
                    "base_node_host": {"type": "string"},
                    "base_node_network_path": {"enum": ["local", "remote", "unknown"]},
                    "authority_host": {"type": "string"},
                    "authority_network_path": {"enum": ["local", "remote", "unknown"]},
                    "mode1_base_node_service_peer": {"type": "string"}
                }
            },
            "config": {
                "type": "object",
                "required": [
                    "A_fund", "C_min", "volume_target", "doubling_rounds",
                    "fanout_outputs_per_tx", "concurrent_batches", "S4_T_budget_secs",
                    "S5_M", "S5_K", "fee_rate", "scan_repetitions",
                    "mode3_batch_creator_sleep_secs",
                    "mode3_unsigned_tx_creator_sleep_secs",
                    "mode3_transaction_signer_sleep_secs",
                    "mode3_broadcaster_sleep_secs",
                    "mode3_confirmation_checker_sleep_secs",
                    "mode3_payment_receiver_scan_interval_secs",
                    "protocol_fingerprint", "seed_fingerprints", "build_manifest",
                    "harness_executable_sha256", "export_harness_executable_sha256"
                ],
                "properties": {
                    "A_fund": {"type": "string"},
                    "C_min": {"type": "integer", "minimum": 1},
                    "volume_target": {"type": "integer", "minimum": 1},
                    "doubling_rounds": {"type": "integer", "minimum": 1},
                    "fanout_outputs_per_tx": {"type": "integer", "minimum": 1},
                    "concurrent_batches": {
                        "type": "array", "items": {"type": "integer", "minimum": 1},
                        "minItems": 1, "uniqueItems": true
                    },
                    "S4_T_budget_secs": {"type": "integer", "minimum": 1},
                    "S5_M": {"type": "integer", "minimum": 1},
                    "S5_K": {"type": "integer", "minimum": 1},
                    "fee_rate": {"type": "string"},
                    "scan_repetitions": {"type": "integer", "minimum": 1},
                    "mode3_batch_creator_sleep_secs": {"type": "integer", "minimum": 1},
                    "mode3_unsigned_tx_creator_sleep_secs": {"type": "integer", "minimum": 1},
                    "mode3_transaction_signer_sleep_secs": {"type": "integer", "minimum": 1},
                    "mode3_broadcaster_sleep_secs": {"type": "integer", "minimum": 1},
                    "mode3_confirmation_checker_sleep_secs": {"type": "integer", "minimum": 1},
                    "mode3_payment_receiver_scan_interval_secs": {"type": "integer", "minimum": 1},
                    "protocol_fingerprint": {"type": "string", "minLength": 64},
                    "seed_fingerprints": {"$ref": "#/$defs/seed_fingerprints"},
                    "build_manifest": {"anyOf": [{"$ref": "#/$defs/build_manifest"}, {"type": "null"}]},
                    "harness_executable_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
                    "export_harness_executable_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"}
                },
                "additionalProperties": true
            },
            "seed_fingerprints": {
                "type": "object", "additionalProperties": false,
                "required": ["old_wallet", "new_wallet", "payment_processor", "recipient_sink"],
                "properties": {
                    "old_wallet": {"type": "string", "minLength": 1},
                    "new_wallet": {"type": "string", "minLength": 1},
                    "payment_processor": {"type": "string", "minLength": 1},
                    "recipient_sink": {"type": "string", "minLength": 1}
                }
            },
            "funding_record": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "amount", "tx_id", "height", "birthday", "birthday_height",
                    "birthday_offset_days", "birthday_start_height"
                ],
                "properties": {
                    "amount": {"type": "string"},
                    "tx_id": {"type": "string", "minLength": 1},
                    "height": {"type": "integer", "minimum": 0},
                    "birthday": {"$ref": "#/$defs/nullable_integer"},
                    "birthday_height": {"$ref": "#/$defs/nullable_integer"},
                    "birthday_offset_days": {"$ref": "#/$defs/nullable_integer"},
                    "birthday_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "construction_ms": {"$ref": "#/$defs/nullable_integer"},
                    "broadcast_to_mempool_ms": {"$ref": "#/$defs/nullable_integer"},
                    "broadcast_to_mempool_unavailable_reason": {"$ref": "#/$defs/nullable_string"},
                    "broadcast_to_confirmed_at_c_min_ms": {"$ref": "#/$defs/nullable_integer"},
                    "tip_height_at_broadcast": {"$ref": "#/$defs/nullable_integer"},
                    "tip_height_at_confirmation": {"$ref": "#/$defs/nullable_integer"},
                    "shared_funding_fee_microtari": {"$ref": "#/$defs/nullable_integer"},
                    "funding_fee_attribution": {"$ref": "#/$defs/nullable_string"}
                }
            },
            "funding": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "old_wallet": {"$ref": "#/$defs/funding_record"},
                    "new_wallet": {"$ref": "#/$defs/funding_record"},
                    "payment_processor": {"$ref": "#/$defs/funding_record"}
                }
            },
            "execution_status": {
                "enum": ["completed", "blocked_prerequisite", "harness_error", "not_applicable"]
            },
            "outcome_status": {"enum": ["success", "partial", "failure", "unavailable"]},
            "repetition": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "run", "execution_status", "outcome_status", "wall_ms", "success_count",
                    "failure_count", "fee_microtari", "error"
                ],
                "properties": {
                    "run": {"type": "integer", "minimum": 1},
                    "execution_status": {"$ref": "#/$defs/execution_status"},
                    "outcome_status": {"$ref": "#/$defs/outcome_status"},
                    "wall_ms": {"$ref": "#/$defs/nullable_integer"},
                    "success_count": {"type": "integer", "minimum": 0},
                    "failure_count": {"type": "integer", "minimum": 0},
                    "fee_microtari": {"$ref": "#/$defs/nullable_integer"},
                    "error": {"$ref": "#/$defs/nullable_string"},
                    "metrics": {
                        "type": "object", "additionalProperties": true,
                        "properties": {
                            "blocked_prerequisite": {"type": "boolean"},
                            "unspent_after": {"$ref": "#/$defs/nullable_integer"},
                            "tx_timings": {"type": "array", "items": {"type": "object", "additionalProperties": true}},
                            "transaction_observations": {
                                "type": "array", "items": {"$ref": "#/$defs/transaction_observation"}
                            },
                            "s4_arms": {"type": "array", "items": {"$ref": "#/$defs/s4_arm"}},
                            "s5_arms": {"type": "object", "additionalProperties": true}
                        }
                    }
                }
            },
            "transaction_observation": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "transaction_id", "attempt_index", "batch_index", "submit_offset_ms",
                    "construction_complete_offset_ms", "construction_ms", "submission_ms",
                    "submission_started", "mempool_available", "mempool_reason", "confirmation_ms",
                    "confirmation_timing_reason", "fee_microtari", "terminal_outcome", "error",
                    "mined_height", "tip_start_height", "tip_end_height", "input_count",
                    "total_output_count", "payment_output_count", "change_output_count",
                    "output_commitments", "configured_batch"
                ],
                "properties": {
                    "transaction_id": {"$ref": "#/$defs/nullable_string"},
                    "attempt_index": {"$ref": "#/$defs/nullable_integer"},
                    "batch_index": {"$ref": "#/$defs/nullable_integer"},
                    "batch_id": {"$ref": "#/$defs/nullable_string"},
                    "submit_offset_ms": {"$ref": "#/$defs/nullable_integer"},
                    "construction_complete_offset_ms": {"$ref": "#/$defs/nullable_integer"},
                    "broadcast_start_offset_ms": {"$ref": "#/$defs/nullable_integer"},
                    "construction_ms": {"$ref": "#/$defs/nullable_integer"},
                    "construction_timing_origin": {"$ref": "#/$defs/nullable_string"},
                    "construction_timing_reason": {"$ref": "#/$defs/nullable_string"},
                    "submission_ms": {"$ref": "#/$defs/nullable_integer"},
                    "submission_timing_origin": {"$ref": "#/$defs/nullable_string"},
                    "submission_started": {"type": ["boolean", "null"]},
                    "mempool_available": {"type": ["boolean", "null"]},
                    "mempool_reason": {"$ref": "#/$defs/nullable_string"},
                    "confirmation_ms": {"$ref": "#/$defs/nullable_integer"},
                    "confirmation_timing_origin": {"$ref": "#/$defs/nullable_string"},
                    "confirmation_timing_reason": {"$ref": "#/$defs/nullable_string"},
                    "amount_microtari": {"$ref": "#/$defs/nullable_integer"},
                    "fee_microtari": {"$ref": "#/$defs/nullable_integer"},
                    "fee_unavailable_reason": {"$ref": "#/$defs/nullable_string"},
                    "fee_disposition": {"enum": ["confirmed_paid", "proposed_unresolved", "rejected", "unavailable"]},
                    "recipient": {"$ref": "#/$defs/nullable_string"},
                    "recipients": {"type": "array", "items": {"type": "string"}},
                    "api_accepted": {"type": ["boolean", "null"]},
                    "api_error": {"$ref": "#/$defs/nullable_string"},
                    "http_status": {"type": ["integer", "null"], "minimum": 100, "maximum": 599},
                    "failure_class": {"enum": ["http_response", "request_timeout", "arm_deadline", "transport", "response_decode", "response_shape", "database", "process", "chain_target", "wallet"]},
                    "deadline_expired": {"type": "boolean"},
                    "terminal_outcome": {"enum": ["confirmed", "rejected", "stalled", "timed_out", "unavailable"]},
                    "error": {"$ref": "#/$defs/nullable_string"},
                    "mined_height": {"$ref": "#/$defs/nullable_integer"},
                    "confirmations": {"$ref": "#/$defs/nullable_integer"},
                    "min_confirmations": {"$ref": "#/$defs/nullable_integer"},
                    "tip_start_height": {"$ref": "#/$defs/nullable_integer"},
                    "tip_end_height": {"$ref": "#/$defs/nullable_integer"},
                    "input_count": {"$ref": "#/$defs/nullable_integer"},
                    "total_output_count": {"$ref": "#/$defs/nullable_integer"},
                    "payment_output_count": {"$ref": "#/$defs/nullable_integer"},
                    "change_output_count": {"$ref": "#/$defs/nullable_integer"},
                    "output_commitments": {"type": "array", "items": {"type": "string"}},
                    "configured_batch": {"$ref": "#/$defs/nullable_integer"}
                }
            },
            "s4_arm": {
                "type": "object", "additionalProperties": false,
                "required": ["configured_batch", "wall_ms", "attempted", "accepted", "confirmed", "rejected", "stalled", "timed_out", "confirmed_success_rate", "api_acceptance_rate", "max_serialization_gap_ms", "serialization_timing_reason", "recipients", "distinct_recipient_count", "recipients_are_distinct"],
                "properties": {
                    "configured_batch": {"type": "integer", "minimum": 1},
                    "wall_ms": {"type": "integer", "minimum": 0},
                    "attempted": {"type": "integer", "minimum": 0},
                    "accepted": {"type": "integer", "minimum": 0},
                    "confirmed": {"type": "integer", "minimum": 0},
                    "rejected": {"type": "integer", "minimum": 0},
                    "stalled": {"type": "integer", "minimum": 0},
                    "timed_out": {"type": "integer", "minimum": 0},
                    "confirmed_success_rate": {"type": ["number", "null"], "minimum": 0, "maximum": 1},
                    "api_acceptance_rate": {"type": ["number", "null"], "minimum": 0, "maximum": 1},
                    "max_serialization_gap_ms": {"$ref": "#/$defs/nullable_integer"},
                    "serialization_timing_reason": {"$ref": "#/$defs/nullable_string"},
                    "recipients": {"type": "array", "items": {"type": "string"}},
                    "distinct_recipient_count": {"type": "integer", "minimum": 0},
                    "recipients_are_distinct": {"type": "boolean"}
                }
            },
            "scenario": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "scenario", "surface", "execution_status", "outcome_status", "repetitions",
                    "median_wall_ms", "spread_wall_ms", "all_runs_median_wall_ms", "notes"
                ],
                "properties": {
                    "scenario": {"enum": ["b0", "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7"]},
                    "surface": {"type": "string", "minLength": 1},
                    "execution_status": {"$ref": "#/$defs/execution_status"},
                    "outcome_status": {"$ref": "#/$defs/outcome_status"},
                    "repetitions": {"type": "array", "items": {"$ref": "#/$defs/repetition"}},
                    "median_wall_ms": {"$ref": "#/$defs/nullable_integer"},
                    "spread_wall_ms": {"$ref": "#/$defs/nullable_integer"},
                    "all_runs_median_wall_ms": {"$ref": "#/$defs/nullable_integer"},
                    "notes": {"type": "array", "items": {"type": "string"}}
                }
            },
            "scenario_map": {
                "type": "object", "additionalProperties": false,
                "required": ["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"],
                "properties": {
                    "B0": {"$ref": "#/$defs/scenario"}, "S0": {"$ref": "#/$defs/scenario"},
                    "S1": {"$ref": "#/$defs/scenario"}, "S2": {"$ref": "#/$defs/scenario"},
                    "S3": {"$ref": "#/$defs/scenario"}, "S4": {"$ref": "#/$defs/scenario"},
                    "S5": {"$ref": "#/$defs/scenario"}, "S6": {"$ref": "#/$defs/scenario"},
                    "S7": {"$ref": "#/$defs/scenario"}
                }
            },
            "mode": {
                "type": "object", "additionalProperties": false,
                "required": ["mode", "address", "scenarios"],
                "properties": {
                    "mode": {"enum": ["old_wallet", "new_wallet", "payment_processor"]},
                    "address": {"$ref": "#/$defs/nullable_string"},
                    "scenarios": {"$ref": "#/$defs/scenario_map"}
                }
            },
            "modes": {
                "type": "object", "additionalProperties": false,
                "required": ["old_wallet", "new_wallet", "payment_processor"],
                "properties": {
                    "old_wallet": {"$ref": "#/$defs/mode"},
                    "new_wallet": {"$ref": "#/$defs/mode"},
                    "payment_processor": {"$ref": "#/$defs/mode"}
                }
            },
            "computed_deltas": {
                "type": "object", "additionalProperties": false,
                "required": ["scan_deltas", "s5_throughput"],
                "properties": {
                    "scan_deltas": {"type": "object", "additionalProperties": true},
                    "s5_throughput": {
                        "type": "object", "additionalProperties": false,
                        "required": ["arms", "comparisons", "comparison_unavailable_reasons", "source"],
                        "properties": {
                            "arms": {"type": "object", "additionalProperties": true},
                            "comparisons": {"type": "object", "additionalProperties": {"type": ["number", "null"]}},
                            "comparison_unavailable_reasons": {"type": "object", "additionalProperties": {"type": ["string", "null"]}},
                            "source": {"type": "string"}
                        }
                    }
                }
            },
            "finding": {
                "type": "object", "additionalProperties": false,
                "required": ["id", "title", "status", "recommendation"],
                "properties": {
                    "id": {"type": "string"}, "title": {"type": "string"},
                    "status": {"type": "string"}, "recommendation": {"type": "string"}
                }
            },
            "verified_transaction": {
                "type": "object", "additionalProperties": false,
                "required": [
                    "tx_id", "status_value", "mode", "scenario", "mined_height", "confirmations",
                    "min_confirmations", "tip_height", "confirmed"
                ],
                "properties": {
                    "tx_id": {"type": "string", "minLength": 1},
                    "status_value": {"const": TX_MINED_CONFIRMED_STATUS},
                    "mode": {"enum": ["old_wallet", "new_wallet", "payment_processor"]},
                    "scenario": {"enum": ["S1", "S4", "S5"]},
                    "amount_microtari": {"type": "integer", "minimum": 0},
                    "fee_microtari": {"type": "integer", "minimum": 0},
                    "mined_height": {"type": "integer", "minimum": 1},
                    "confirmations": {"type": "integer", "minimum": 1},
                    "min_confirmations": {"type": "integer", "minimum": 1},
                    "tip_height": {"type": "integer", "minimum": 1},
                    "confirmed": {"const": true}
                }
            },
            "chain_verification": {
                "type": "object", "additionalProperties": false,
                "required": ["tx_mined_confirmed_status_value", "verified_transactions"],
                "properties": {
                    "tx_mined_confirmed_status_value": {"const": TX_MINED_CONFIRMED_STATUS},
                    "verified_transactions": {"type": "array", "items": {"$ref": "#/$defs/verified_transaction"}}
                }
            }
        }
    })
}

fn legacy_v6_schema_document() -> Value {
    let mut schema = schema_document();
    schema["properties"]["schema_version"]["const"] = json!(LEGACY_RESULT_SCHEMA_VERSION);

    for (definition, fields) in [
        (
            "provenance",
            &[
                "measurement_worktree_clean",
                "export_worktree_clean",
                "measurement_build_worktree_clean",
                "export_build_worktree_clean",
            ][..],
        ),
        ("base_node", &["pruned_height"][..]),
        (
            "funding_record",
            &[
                "birthday",
                "birthday_height",
                "birthday_offset_days",
                "birthday_start_height",
            ][..],
        ),
        (
            "config",
            &[
                "seed_fingerprints",
                "mode3_batch_creator_sleep_secs",
                "mode3_unsigned_tx_creator_sleep_secs",
                "mode3_transaction_signer_sleep_secs",
                "mode3_broadcaster_sleep_secs",
                "mode3_confirmation_checker_sleep_secs",
                "mode3_payment_receiver_scan_interval_secs",
                "harness_executable_sha256",
                "export_harness_executable_sha256",
            ][..],
        ),
        ("transaction_observation", &["submission_started"][..]),
    ] {
        let required = schema["$defs"][definition]["required"]
            .as_array_mut()
            .expect("schema definition has a required array");
        required.retain(|field| field.as_str().is_none_or(|field| !fields.contains(&field)));
    }

    schema["$defs"]["base_node"]["properties"]
        .as_object_mut()
        .expect("base_node schema properties")
        .remove("pruned_height");
    for field in [
        "measurement_worktree_clean",
        "export_worktree_clean",
        "measurement_build_worktree_clean",
        "export_build_worktree_clean",
    ] {
        schema["$defs"]["provenance"]["properties"]
            .as_object_mut()
            .expect("provenance schema properties")
            .remove(field);
    }
    for field in ["birthday_height", "birthday_offset_days"] {
        schema["$defs"]["funding_record"]["properties"]
            .as_object_mut()
            .expect("funding_record schema properties")
            .remove(field);
    }
    schema["$defs"]["config"]["properties"]
        .as_object_mut()
        .expect("config schema properties")
        .remove("seed_fingerprints");
    for field in [
        "harness_executable_sha256",
        "export_harness_executable_sha256",
    ] {
        schema["$defs"]["config"]["properties"]
            .as_object_mut()
            .expect("config schema properties")
            .remove(field);
    }
    schema["$defs"]
        .as_object_mut()
        .expect("schema definitions")
        .remove("seed_fingerprints");
    schema["$defs"]["build_manifest"] = json!({
        "type": "object", "additionalProperties": false,
        "required": ["schema_version", "artifacts"],
        "properties": {
            "schema_version": {"type": "integer", "minimum": 1},
            "sources": {"type": "object"},
            "artifacts": {
                "type": "object", "minProperties": 1,
                "additionalProperties": {"$ref": "#/$defs/build_artifact"}
            },
            "payment_processor_patch_sha256": {"type": "string", "minLength": 1}
        }
    });
    schema["$defs"]["build_artifact"] = json!({
        "type": "object", "additionalProperties": true,
        "required": ["source_revision", "sha256"],
        "properties": {
            "source_revision": {"type": "string", "minLength": 1},
            "sha256": {"type": "string", "minLength": 1}
        }
    });
    schema
}

pub fn validate_path(path: &Path, submission: bool) -> anyhow::Result<ResultProfile> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", path.display()))?;
    validate_document(&document, submission)
}

pub fn validate_legacy_v5_path(path: &Path) -> anyhow::Result<()> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", path.display()))?;
    validate_legacy_v5_document(&document)
}

pub fn validate_legacy_v6_path(path: &Path) -> anyhow::Result<ResultProfile> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", path.display()))?;
    validate_legacy_v6_document(&document)
}

fn validate_legacy_v5_document(document: &Value) -> anyhow::Result<()> {
    if document["schema_version"] != json!(5) {
        bail!("legacy validation requires schema_version 5");
    }
    if document["profile_kind"] != json!("final") || document["run_complete"] != json!(true) {
        bail!("legacy schema-v5 profile must be a complete final profile");
    }
    for key in [
        "run_id",
        "harness_git_commit",
        "generated_at",
        "network",
        "base_node",
        "environment",
        "versions",
        "config",
        "funding",
        "modes",
        "computed_deltas",
        "findings",
        "chain_verification",
    ] {
        if document.get(key).is_none() {
            bail!("legacy schema-v5 profile is missing {key}");
        }
    }
    let modes = document["modes"]
        .as_object()
        .context("legacy schema-v5 modes must be an object")?;
    for mode in ModeName::ALL {
        let scenarios = modes
            .get(mode.as_str())
            .and_then(|value| value["scenarios"].as_object())
            .with_context(|| format!("legacy schema-v5 missing {}/scenarios", mode.as_str()))?;
        for scenario in ScenarioName::ALL {
            if !scenarios.contains_key(scenario.as_str()) {
                bail!(
                    "legacy schema-v5 missing {}/{}",
                    mode.as_str(),
                    scenario.as_str()
                );
            }
        }
    }
    Ok(())
}

fn validate_legacy_v6_document(document: &Value) -> anyhow::Result<ResultProfile> {
    let schema = legacy_v6_schema_document();
    validate_schema_with(
        document,
        &schema,
        &format!("legacy schema v{LEGACY_RESULT_SCHEMA_VERSION}"),
    )?;
    let profile: ResultProfile = serde_json::from_value(document.clone())
        .context("deserializing legacy schema-v6 result profile")?;
    validate_identity_for_version(&profile, LEGACY_RESULT_SCHEMA_VERSION)?;
    validate_transactions(&profile)?;
    validate_status_pairs(document)?;
    if profile.computed_deltas != super::computed_deltas(&profile) {
        bail!("computed_deltas do not match recomputed source scenario metrics");
    }
    if profile.profile_kind == ProfileKind::Checkpoint && profile.run_complete {
        bail!("checkpoint profile must have run_complete=false");
    }
    if profile.profile_kind == ProfileKind::Final {
        validate_final_document(&profile, document, false)?;
    }
    Ok(profile)
}

pub fn validate_document(document: &Value, submission: bool) -> anyhow::Result<ResultProfile> {
    validate_schema(document)?;
    let profile: ResultProfile = serde_json::from_value(document.clone())
        .with_context(|| format!("deserializing schema-v{RESULT_SCHEMA_VERSION} result profile"))?;
    validate_profile(&profile, submission)?;
    if profile.profile_kind == ProfileKind::Final || submission {
        validate_final_document(&profile, document, submission)?;
    }
    Ok(profile)
}

pub fn validate_profile(profile: &ResultProfile, _submission: bool) -> anyhow::Result<()> {
    let document = serde_json::to_value(profile).context("serializing result profile")?;
    validate_schema(&document)?;
    validate_identity(profile)?;
    validate_transactions(profile)?;
    validate_status_pairs(&document)?;
    if profile.computed_deltas != super::computed_deltas(profile) {
        bail!("computed_deltas do not match recomputed source scenario metrics");
    }
    if profile.profile_kind == ProfileKind::Checkpoint && profile.run_complete {
        bail!("checkpoint profile must have run_complete=false");
    }
    Ok(())
}

pub fn validate_final(profile: &ResultProfile, submission: bool) -> anyhow::Result<()> {
    let document = serde_json::to_value(profile).context("serializing result profile")?;
    validate_final_document(profile, &document, submission)
}

fn validate_schema(document: &Value) -> anyhow::Result<()> {
    let schema = schema_document();
    validate_schema_with(
        document,
        &schema,
        &format!("schema v{RESULT_SCHEMA_VERSION}"),
    )
}

fn validate_schema_with(document: &Value, schema: &Value, label: &str) -> anyhow::Result<()> {
    let validator = jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(schema)
        .context("compiling result profile JSON Schema")?;
    let errors = validator
        .iter_errors(document)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        bail!("profile does not match {label}:\n{}", errors.join("\n"));
    }
    Ok(())
}

fn validate_identity(profile: &ResultProfile) -> anyhow::Result<()> {
    validate_identity_for_version(profile, RESULT_SCHEMA_VERSION)
}

fn validate_identity_for_version(
    profile: &ResultProfile,
    expected_version: u32,
) -> anyhow::Result<()> {
    if profile.schema_version != expected_version {
        bail!("schema_version must be {expected_version}");
    }
    for mode in ModeName::ALL {
        let key = mode.as_str();
        let entry = profile
            .modes
            .get(key)
            .with_context(|| format!("missing mode {key}"))?;
        if entry.mode != mode {
            bail!("mode map key {key} does not match embedded mode");
        }
        for scenario in ScenarioName::ALL {
            let scenario_key = scenario.as_str();
            let cell = entry
                .scenarios
                .get(scenario_key)
                .with_context(|| format!("missing {key}/{scenario_key}"))?;
            if cell.scenario != scenario {
                bail!("scenario map key {key}/{scenario_key} does not match embedded scenario");
            }
        }
    }
    Ok(())
}

fn validate_transactions(profile: &ResultProfile) -> anyhow::Result<()> {
    if profile.chain_verification.tx_mined_confirmed_status_value != TX_MINED_CONFIRMED_STATUS {
        bail!("chain verification status constant must be {TX_MINED_CONFIRMED_STATUS}");
    }
    let mut seen = BTreeSet::new();
    for tx in &profile.chain_verification.verified_transactions {
        if tx.status_value != TX_MINED_CONFIRMED_STATUS || !tx.confirmed {
            bail!(
                "top-level transaction {} is not independently confirmed",
                tx.tx_id
            );
        }
        if tx.mined_height.is_none() {
            bail!("top-level transaction {} has no mined_height", tx.tx_id);
        }
        let confirmations = tx
            .confirmations
            .context("top-level transaction has no confirmations")?;
        let min_confirmations = tx
            .min_confirmations
            .context("top-level transaction has no min_confirmations")?;
        let tip_height = tx
            .tip_height
            .context("top-level transaction has no tip_height")?;
        let mined_height = tx.mined_height.expect("checked above");
        if confirmations < min_confirmations
            || tip_height < mined_height
            || tip_height.saturating_sub(mined_height) < min_confirmations
        {
            bail!(
                "top-level transaction {} has insufficient independent C_min depth proof",
                tx.tx_id
            );
        }
        if !seen.insert(&tx.tx_id) {
            bail!("duplicate top-level transaction id {}", tx.tx_id);
        }
    }
    Ok(())
}

fn validate_status_pairs(document: &Value) -> anyhow::Result<()> {
    for (mode, scenarios) in mode_scenarios(document)? {
        for (scenario, cell) in scenarios {
            validate_status_pair(cell, &format!("{mode}/{scenario}"))?;
            if let Some(repetitions) = cell.get("repetitions").and_then(Value::as_array) {
                for repetition in repetitions {
                    validate_status_pair(repetition, &format!("{mode}/{scenario} repetition"))?;
                    if repetition["execution_status"] == "blocked_prerequisite"
                        && (!repetition["wall_ms"].is_null()
                            || repetition["success_count"] != 0
                            || repetition["failure_count"] != 0
                            || !repetition["fee_microtari"].is_null()
                            || !repetition["error"].is_null())
                    {
                        bail!(
                            "blocked repetition {mode}/{scenario} must not contain measured wall, fee, outcome counts, or error"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_status_pair(value: &Value, label: &str) -> anyhow::Result<()> {
    let execution = value["execution_status"].as_str().unwrap_or_default();
    let outcome = value["outcome_status"].as_str().unwrap_or_default();
    let coherent = match execution {
        "completed" => matches!(outcome, "success" | "partial" | "failure"),
        "blocked_prerequisite" | "harness_error" | "not_applicable" => outcome == "unavailable",
        _ => false,
    };
    if !coherent {
        bail!("incoherent execution/outcome status for {label}: {execution}/{outcome}");
    }
    Ok(())
}

fn validate_final_document(
    profile: &ResultProfile,
    document: &Value,
    submission: bool,
) -> anyhow::Result<()> {
    if profile.profile_kind != ProfileKind::Final || !profile.run_complete {
        bail!("final profile must have profile_kind=final and run_complete=true");
    }
    for (mode, scenarios) in mode_scenarios(document)? {
        for (scenario, cell) in scenarios {
            let execution = cell["execution_status"].as_str().unwrap_or_default();
            let repetitions = cell["repetitions"].as_array().expect("schema checked");
            if repetitions.is_empty() && execution != "not_applicable" {
                bail!("final profile contains unexecuted cell {mode}/{scenario}");
            }
        }
    }
    validate_s1_exact_outputs(document)?;
    validate_s5_comparisons(document)?;
    if submission {
        validate_reference_configuration(profile, document)?;
    }
    Ok(())
}

fn validate_reference_configuration(
    profile: &ResultProfile,
    document: &Value,
) -> anyhow::Result<()> {
    let config = &document["config"];
    reference_s5_per_recipient_amount(config)?;
    let exact = [
        ("A_fund", json!("10000 T")),
        ("C_min", json!(3)),
        ("volume_target", json!(512)),
        ("doubling_rounds", json!(6)),
        ("fanout_outputs_per_tx", json!(8)),
        ("S4_T_budget_secs", json!(900)),
        ("S5_M", json!(100)),
        ("S5_K", json!(10)),
        ("fee_rate", json!("5 uT")),
        (
            "mode3_batch_creator_sleep_secs",
            json!(crate::config::MODE3_BATCH_CREATOR_SLEEP_SECS),
        ),
        (
            "mode3_unsigned_tx_creator_sleep_secs",
            json!(crate::config::MODE3_UNSIGNED_TX_CREATOR_SLEEP_SECS),
        ),
        (
            "mode3_transaction_signer_sleep_secs",
            json!(crate::config::MODE3_TRANSACTION_SIGNER_SLEEP_SECS),
        ),
        (
            "mode3_broadcaster_sleep_secs",
            json!(crate::config::MODE3_BROADCASTER_SLEEP_SECS),
        ),
        (
            "mode3_confirmation_checker_sleep_secs",
            json!(crate::config::MODE3_CONFIRMATION_CHECKER_SLEEP_SECS),
        ),
        (
            "mode3_payment_receiver_scan_interval_secs",
            json!(crate::config::MODE3_PAYMENT_RECEIVER_SCAN_INTERVAL_SECS),
        ),
    ];
    for (key, expected) in exact {
        if config[key] != expected {
            bail!(
                "submission config {key} must be {expected}, got {}",
                config[key]
            );
        }
    }
    if config["concurrent_batches"] != json!(REFERENCE_S4_RAMP) {
        bail!("submission config must contain the full S4 concurrency ramp");
    }
    if config["scenario_order"] != json!(["B0", "S0", "S1", "S2", "S3", "S4", "S5", "S6", "S7"]) {
        bail!("submission config must record canonical B0,S0-S7 scenario order");
    }
    validate_cross_cutting_scenario_metrics(document)?;
    validate_scenario_specific_metrics(document)?;
    for (mode, scenarios) in mode_scenarios(document)? {
        if scenarios["B0"]["execution_status"] != "completed"
            || !matches!(
                scenarios["B0"]["outcome_status"].as_str(),
                Some("success" | "failure")
            )
        {
            bail!("submission requires completed B0 evidence for {mode}");
        }
        if scenarios["S0"]["execution_status"] != "completed"
            || scenarios["S0"]["outcome_status"] != "success"
        {
            bail!("submission requires successful S0 for {mode}");
        }
        for (scenario, cell) in scenarios {
            if cell["execution_status"] == "not_applicable" {
                bail!(
                    "submission requires all 27 benchmark cells; {mode}/{scenario} is not applicable"
                );
            }
            if cell["execution_status"] == "harness_error" {
                bail!("submission contains harness error at {mode}/{scenario}");
            }
            if cell["repetitions"].as_array().is_some_and(|runs| {
                runs.iter()
                    .any(|run| run["execution_status"] == "harness_error")
            }) {
                bail!("submission contains harness error repetition at {mode}/{scenario}");
            }
        }
    }
    for flag in [
        "mode1_live_topology",
        "mode2_live_scenarios",
        "mode3_live_topology",
        "live_fresh_scan_cells",
    ] {
        if config[flag] != Value::Bool(true) {
            bail!("submission config {flag} must be true");
        }
    }
    for cap in [
        "mode1_live_max_s1_txs",
        "mode1_live_max_s4_batch",
        "mode1_live_max_s5_items",
        "mode2_live_max_s1_txs",
        "mode2_live_max_s4_batch",
        "mode2_live_max_s5_txs",
        "mode3_live_max_s1_batches",
        "mode3_live_max_s4_batch",
        "mode3_live_max_s5_items",
    ] {
        if config[cap] != json!(0) {
            bail!("submission config {cap} must be zero (uncapped)");
        }
    }
    let completed = profile
        .completed_stages
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if REQUIRED_STAGES
        .iter()
        .any(|stage| !completed.contains(stage))
    {
        bail!("submission profile is missing one or more completed stages");
    }
    validate_submission_harness_provenance(profile)?;
    validate_submission_build_manifests(profile)?;
    let seed_fingerprints = profile.config["seed_fingerprints"]
        .as_object()
        .context("submission profile must record seed fingerprints")?;
    for role in [
        "old_wallet",
        "new_wallet",
        "payment_processor",
        "recipient_sink",
    ] {
        if seed_fingerprints
            .get(role)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            bail!("submission profile is missing the {role} seed fingerprint");
        }
    }
    if profile.environment.authority_network_path != "remote" {
        bail!("submission requires an independent remote Esmeralda authority endpoint");
    }
    if profile.environment.base_node_network_path != "local"
        || profile.environment.mode1_base_node_service_peer.is_none()
    {
        bail!("submission requires an archival local scan endpoint and Mode 1 P2P service peer");
    }
    if profile
        .environment
        .disk_kind
        .as_deref()
        .is_none_or(str::is_empty)
        || profile
            .environment
            .disk_name
            .as_deref()
            .is_none_or(str::is_empty)
    {
        bail!("submission requires disk kind and device evidence for the candidate data namespace");
    }
    if profile.base_node.configured_revision != REFERENCE_BASE_NODE_REVISION {
        bail!(
            "submission base-node compatibility reference must be {REFERENCE_BASE_NODE_REVISION}"
        );
    }
    if !profile.base_node.version_observable && profile.base_node.observed_version.is_some() {
        bail!("unobservable base-node version must not claim an observed version");
    }
    if profile.base_node.tip_start_height.is_none()
        || profile
            .base_node
            .tip_start_hash
            .as_deref()
            .is_none_or(str::is_empty)
        || profile.base_node.tip_end_height.is_none()
        || profile
            .base_node
            .tip_end_hash
            .as_deref()
            .is_none_or(str::is_empty)
        || profile.base_node.authority_tip_start_height.is_none()
        || profile
            .base_node
            .authority_tip_start_hash
            .as_deref()
            .is_none_or(str::is_empty)
        || profile.base_node.authority_tip_end_height.is_none()
        || profile
            .base_node
            .authority_tip_end_hash
            .as_deref()
            .is_none_or(str::is_empty)
    {
        bail!("submission requires complete selected-node and authority tip evidence");
    }
    if profile.base_node.is_synced != Some(true)
        || profile.base_node.pruning_horizon != Some(0)
        || profile.base_node.pruned_height != Some(0)
    {
        bail!(
            "submission profile must record is_synced=true, pruning_horizon=0, and pruned_height=0"
        );
    }
    if profile
        .chain_verification
        .verified_transactions
        .iter()
        .any(|tx| tx.fee_microtari.is_none())
    {
        bail!("submission top-level transactions must include observed fees");
    }
    validate_confirmed_observation_bindings(document, profile)?;
    for mode in ModeName::ALL {
        if profile.modes[mode.as_str()].address.is_none() {
            bail!(
                "submission mode {} is missing its public address",
                mode.as_str()
            );
        }
        let Some(funding) = profile.funding.get(mode.as_str()) else {
            bail!("submission funding is missing {}", mode.as_str());
        };
        let birthday = funding.birthday.with_context(|| {
            format!(
                "submission funding for {} is missing birthday",
                mode.as_str()
            )
        })?;
        let birthday_height = funding.birthday_height.with_context(|| {
            format!(
                "submission funding for {} is missing birthday_height",
                mode.as_str()
            )
        })?;
        let birthday_start_height = funding.birthday_start_height.with_context(|| {
            format!(
                "submission funding for {} is missing birthday_start_height",
                mode.as_str()
            )
        })?;
        let birthday_offset_days = funding.birthday_offset_days.with_context(|| {
            format!(
                "submission funding for {} is missing birthday_offset_days",
                mode.as_str()
            )
        })?;
        if birthday == 0
            || birthday_offset_days != 2
            || birthday < birthday_offset_days
            || birthday_start_height > birthday_height
            || birthday_height > funding.height
        {
            bail!(
                "submission funding for {} has an invalid stock-wallet birthday resolution",
                mode.as_str()
            );
        }
        if funding.construction_ms.is_none()
            || (funding.broadcast_to_mempool_ms.is_none()
                && funding
                    .broadcast_to_mempool_unavailable_reason
                    .as_deref()
                    .is_none_or(str::is_empty))
            || funding.broadcast_to_confirmed_at_c_min_ms.is_none()
            || funding.tip_height_at_broadcast.is_none()
            || funding.tip_height_at_confirmation.is_none()
        {
            bail!(
                "submission funding for {} is missing measured S0 timing/tip evidence",
                mode.as_str()
            );
        }
        if funding.shared_funding_fee_microtari.is_none()
            || funding.funding_fee_attribution.as_deref()
                != Some("external_source_shared_not_deducted_from_mode_balance")
        {
            bail!(
                "submission funding for {} is missing shared fee attribution",
                mode.as_str()
            );
        }
        let s0 = &profile.modes[mode.as_str()].scenarios["S0"];
        if s0.status == CellStatus::Ok {
            for (repetition, run) in
                document["modes"][mode.as_str()]["scenarios"]["S0"]["repetitions"]
                    .as_array()
                    .expect("schema checked")
                    .iter()
                    .enumerate()
            {
                if run["execution_status"] != "completed" {
                    continue;
                }
                let metrics = run["metrics"].as_object().with_context(|| {
                    format!(
                        "submission S0 repetition {} metrics missing for {}",
                        repetition + 1,
                        mode.as_str()
                    )
                })?;
                if metrics["verification_source"] != json!("wallet_state_observed")
                    || metrics["expected_spendable_count"] != json!(1)
                    || metrics["observed_spendable_count"] != json!(1)
                    || metrics["expected_available_microtari"]
                        != json!(crate::config::parse_amount("10000 T")?.0)
                    || metrics["available_microtari"]
                        != json!(crate::config::parse_amount("10000 T")?.0)
                    || metrics["spendable_count_matches_expected"] != json!(true)
                    || metrics["balance_matches_expected"] != json!(true)
                    || metrics["wallet_state_complete"] != json!(true)
                    || metrics["pending_outputs"] != json!(0)
                    || metrics["locked_outputs"] != json!(0)
                    || metrics["invalid_outputs"] != json!(0)
                    || metrics["unknown_outputs"] != json!(0)
                {
                    bail!(
                        "submission S0 repetition {} for {} does not prove exact one-UTXO A_fund state",
                        repetition + 1,
                        mode.as_str()
                    );
                }
                let funding_observation =
                    metrics.get("funding_observation").with_context(|| {
                        format!(
                            "successful submission S0 repetition {} is missing funding observation",
                            repetition + 1
                        )
                    })?;
                if funding_observation["tx_id"] != json!(funding.tx_id)
                    || funding_observation["mined_height"] != json!(funding.height)
                    || funding_observation["shared_funding_fee_microtari"]
                        != json!(funding.shared_funding_fee_microtari)
                {
                    bail!(
                        "submission S0 repetition {} funding observation does not match funding record for {}",
                        repetition + 1,
                        mode.as_str()
                    );
                }
                if funding_observation["birthday"] != json!(funding.birthday)
                    || funding_observation["H_birth"] != json!(funding.birthday_height)
                    || funding_observation["birthday_height"] != json!(funding.birthday_height)
                    || funding_observation["birthday_offset_days"]
                        != json!(funding.birthday_offset_days)
                    || funding_observation["birthday_start_height"]
                        != json!(funding.birthday_start_height)
                {
                    bail!(
                        "submission S0 repetition {} birthday observation does not match funding record for {}",
                        repetition + 1,
                        mode.as_str()
                    );
                }
            }
        }
        if profile.modes[mode.as_str()].scenarios["S1"].status == CellStatus::Ok
            && !profile
                .chain_verification
                .verified_transactions
                .iter()
                .any(|tx| tx.mode == mode.as_str() && tx.scenario == "S1")
        {
            bail!(
                "successful submission S1 for {} has no independently verified transaction",
                mode.as_str()
            );
        }
    }
    let funding_records = ModeName::ALL
        .into_iter()
        .map(|mode| {
            profile
                .funding
                .get(mode.as_str())
                .with_context(|| format!("submission funding is missing {}", mode.as_str()))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let first = funding_records.first().expect("three benchmark modes");
    let expected_amount = crate::config::parse_amount("10000 T")?.0;
    if funding_records.iter().any(|funding| {
        funding.tx_id != first.tx_id
            || funding.height != first.height
            || funding.amount != "10000 T"
            || crate::config::parse_amount(&funding.amount)
                .map(|amount| amount.0 != expected_amount)
                .unwrap_or(true)
            || funding.shared_funding_fee_microtari != first.shared_funding_fee_microtari
            || funding.tip_height_at_broadcast != first.tip_height_at_broadcast
            || funding.construction_ms != first.construction_ms
            || funding.broadcast_to_mempool_ms != first.broadcast_to_mempool_ms
            || funding.broadcast_to_mempool_unavailable_reason
                != first.broadcast_to_mempool_unavailable_reason
            || funding.broadcast_to_confirmed_at_c_min_ms
                != first.broadcast_to_confirmed_at_c_min_ms
            || funding.tip_height_at_confirmation != first.tip_height_at_confirmation
            || funding.birthday != first.birthday
            || funding.birthday_height != first.birthday_height
            || funding.birthday_offset_days != first.birthday_offset_days
            || funding.birthday_start_height != first.birthday_start_height
    }) {
        bail!("submission funding records must describe one shared exact A_fund transaction");
    }
    Ok(())
}

fn reference_s5_per_recipient_amount(config: &Value) -> anyhow::Result<u64> {
    let amounts = [
        "mode1_payment_amount",
        "mode2_payment_amount",
        "mode3_payment_amount",
    ]
    .map(|key| {
        let value = config[key]
            .as_str()
            .with_context(|| format!("submission config {key} must be an amount string"))?;
        crate::config::parse_amount(value)
            .map(|amount| amount.0)
            .with_context(|| format!("submission config {key} is not a valid amount"))
    });
    let [mode1, mode2, mode3] = amounts;
    let (mode1, mode2, mode3) = (mode1?, mode2?, mode3?);
    if mode1 != mode2 || mode1 != mode3 {
        bail!("submission S5 requires the same per-recipient payment amount for every wallet mode");
    }
    Ok(mode1)
}

fn validate_s5_observation_amounts(
    document: &Value,
    mode: &str,
    metrics: &Value,
) -> anyhow::Result<()> {
    let per_recipient = reference_s5_per_recipient_amount(&document["config"])?;
    let observations = metrics["transaction_observations"]
        .as_array()
        .with_context(|| format!("submission {mode}/S5 missing transaction observations"))?;
    for observation in observations
        .iter()
        .filter(|observation| observation["terminal_outcome"] == "confirmed")
    {
        let recipient_count = observation["recipients"]
            .as_array()
            .with_context(|| {
                format!("submission {mode}/S5 confirmed observation lacks recipients")
            })?
            .len();
        let recipient_count =
            u64::try_from(recipient_count).context("submission S5 recipient count exceeds u64")?;
        let expected = per_recipient
            .checked_mul(recipient_count)
            .context("submission S5 transaction amount overflows u64")?;
        if observation["amount_microtari"].as_u64() != Some(expected) {
            bail!(
                "submission {mode}/S5 confirmed observation amount must equal the canonical per-recipient amount multiplied by its recipient count"
            );
        }
    }
    Ok(())
}

fn validate_confirmed_observation_bindings(
    document: &Value,
    profile: &ResultProfile,
) -> anyhow::Result<()> {
    let mut observed = BTreeMap::<(String, String, String), usize>::new();
    for mode in ModeName::ALL {
        for scenario in ScenarioName::ALL {
            for (repetition, run) in document["modes"][mode.as_str()]["scenarios"]
                [scenario.as_str()]["repetitions"]
                .as_array()
                .expect("schema checked")
                .iter()
                .enumerate()
            {
                let Some(observations) = run["metrics"]
                    .get("transaction_observations")
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                for observation in observations
                    .iter()
                    .filter(|value| value["terminal_outcome"] == "confirmed")
                {
                    let tx_id = observation["transaction_id"].as_str().with_context(|| {
                        format!(
                            "confirmed {}/{} repetition {} observation lacks tx id",
                            mode.as_str(),
                            scenario.as_str(),
                            repetition + 1
                        )
                    })?;
                    let key = (
                        mode.as_str().to_string(),
                        scenario.as_str().to_string(),
                        tx_id.to_string(),
                    );
                    *observed.entry(key).or_default() += 1;
                    let chain_row = profile
                        .chain_verification
                        .verified_transactions
                        .iter()
                        .find(|transaction| {
                            transaction.mode == mode.as_str()
                                && transaction.scenario == scenario.as_str()
                                && transaction.tx_id == tx_id
                        })
                        .with_context(|| {
                            format!(
                                "confirmed {}/{} transaction {} lacks a matching chain row",
                                mode.as_str(),
                                scenario.as_str(),
                                tx_id
                            )
                        })?;
                    for (name, expected, actual) in [
                        (
                            "amount",
                            observation["amount_microtari"].as_u64(),
                            chain_row.amount_microtari,
                        ),
                        (
                            "fee",
                            observation["fee_microtari"].as_u64(),
                            chain_row.fee_microtari,
                        ),
                        (
                            "mined height",
                            observation["mined_height"].as_u64(),
                            chain_row.mined_height,
                        ),
                        (
                            "tip height",
                            observation["tip_end_height"].as_u64(),
                            chain_row.tip_height,
                        ),
                        (
                            "confirmations",
                            observation["confirmations"].as_u64(),
                            chain_row.confirmations,
                        ),
                        (
                            "minimum confirmations",
                            observation["min_confirmations"].as_u64(),
                            chain_row.min_confirmations,
                        ),
                    ] {
                        if expected.is_none() {
                            bail!(
                                "confirmed {}/{} transaction {} omits {} evidence",
                                mode.as_str(),
                                scenario.as_str(),
                                tx_id,
                                name
                            );
                        }
                        if expected != actual {
                            bail!(
                                "confirmed {}/{} transaction {} has mismatched {} evidence",
                                mode.as_str(),
                                scenario.as_str(),
                                tx_id,
                                name
                            );
                        }
                    }
                }
            }
        }
    }
    let mut chain = BTreeMap::<(String, String, String), usize>::new();
    for transaction in &profile.chain_verification.verified_transactions {
        if !transaction.confirmed {
            bail!("top-level chain verification contains an unconfirmed transaction");
        }
        let key = (
            transaction.mode.clone(),
            transaction.scenario.clone(),
            transaction.tx_id.clone(),
        );
        *chain.entry(key).or_default() += 1;
    }
    if observed != chain {
        bail!("confirmed transaction observations and top-level chain rows are not one-to-one");
    }
    Ok(())
}

fn validate_submission_build_manifests(profile: &ResultProfile) -> anyhow::Result<()> {
    let configured = profile
        .config
        .get("build_manifest")
        .context("submission profile must record the configured build manifest")?;
    let values = [
        (
            "measurement",
            &profile.provenance.measurement_build_manifest,
        ),
        ("export", &profile.provenance.export_build_manifest),
        ("configured", configured),
    ];
    let manifests = values
        .into_iter()
        .map(|(label, value)| {
            let manifest: crate::build_manifest::BuildManifest =
                serde_json::from_value(value.clone()).with_context(|| {
                    format!(
                        "submission {label} build manifest must use the exact typed schema-v2 shape"
                    )
                })?;
            crate::build_manifest::verify_portable_manifest(&manifest).with_context(|| {
                format!(
                    "submission {label} build manifest does not match embedded canonical provenance"
                )
            })?;
            Ok(manifest)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if manifests[0] != manifests[1] || manifests[0] != manifests[2] {
        bail!("submission measurement, export, and configured build manifests must be identical");
    }
    Ok(())
}

fn validate_submission_harness_provenance(profile: &ResultProfile) -> anyhow::Result<()> {
    if !is_lowercase_git_commit(&profile.provenance.measurement_commit)
        || !is_lowercase_git_commit(&profile.provenance.export_commit)
    {
        bail!(
            "submission measurement and export commits must be lowercase 40-character hexadecimal"
        );
    }
    if profile.provenance.measurement_commit != profile.provenance.export_commit {
        bail!("submission measurement and export commits must be identical");
    }
    if !profile.provenance.measurement_worktree_clean || !profile.provenance.export_worktree_clean {
        bail!("submission requires explicit clean-worktree evidence at measurement and export");
    }
    if !profile.provenance.measurement_build_worktree_clean
        || !profile.provenance.export_build_worktree_clean
    {
        bail!("submission requires a harness executable built from a clean worktree");
    }
    let measurement_executable = profile
        .config
        .get("harness_executable_sha256")
        .and_then(Value::as_str)
        .context("submission is missing harness_executable_sha256")?;
    let export_executable = profile
        .config
        .get("export_harness_executable_sha256")
        .and_then(Value::as_str)
        .context("submission is missing export_harness_executable_sha256")?;
    if measurement_executable != export_executable {
        bail!("submission measurement and export harness executable SHA-256 values must match");
    }
    Ok(())
}

fn is_lowercase_git_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_cross_cutting_scenario_metrics(document: &Value) -> anyhow::Result<()> {
    for (mode, scenarios) in mode_scenarios(document)? {
        for (scenario, cell) in scenarios {
            for (index, run) in cell["repetitions"]
                .as_array()
                .expect("schema checked")
                .iter()
                .enumerate()
            {
                if run["execution_status"] != "completed" {
                    continue;
                }
                let label = format!("{mode}/{scenario} repetition {}", index + 1);
                if run["wall_ms"].as_u64().is_none() {
                    bail!("submission {label} must record wall_ms");
                }
                if run["fee_microtari"].as_u64().is_none() {
                    bail!("submission {label} must record explicit fees, including zero");
                }
                let Some(metrics) = run["metrics"].as_object() else {
                    bail!("submission {label} must record scenario metrics");
                };
                if let Some(observations) = metrics
                    .get("transaction_observations")
                    .and_then(Value::as_array)
                {
                    let counts = OutcomeCounts::from_json_observations(observations);
                    if metrics.get("outcome_counts") != Some(&serde_json::to_value(&counts)?) {
                        bail!("submission {label} outcome_counts do not match its observations");
                    }
                    let preflight_failures = validate_s1_preflight_failures(
                        mode,
                        scenario,
                        &Value::Object(metrics.clone()),
                    )?;
                    let nonconfirmed = counts
                        .attempted
                        .saturating_sub(counts.confirmed)
                        .saturating_add(u32::try_from(preflight_failures).unwrap_or(u32::MAX));
                    if run["success_count"].as_u64() != Some(u64::from(counts.confirmed))
                        || run["failure_count"]
                            .as_u64()
                            .is_none_or(|failures| failures < u64::from(nonconfirmed))
                        || (run["outcome_status"] == "success"
                            && (nonconfirmed != 0 || run["failure_count"] != 0))
                    {
                        bail!(
                            "submission {label} success/failure aggregates do not match its observations and preflight failures"
                        );
                    }
                }
                let confirmed_fee_total = metrics
                    .get("transaction_observations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|observation| observation["terminal_outcome"] == "confirmed")
                    .map(|observation| {
                        observation["fee_microtari"].as_u64().with_context(|| {
                            format!("submission {label} confirmed observation lacks fee")
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
                    .into_iter()
                    .try_fold(0u64, |total, fee| total.checked_add(fee))
                    .with_context(|| format!("submission {label} confirmed fee total overflows"))?;
                if run["fee_microtari"] != json!(confirmed_fee_total) {
                    bail!(
                        "submission {label} fee_microtari does not equal confirmed observation fees"
                    );
                }
                let confirmed_outgoing_total = metrics
                    .get("transaction_observations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|observation| observation["terminal_outcome"] == "confirmed")
                    .map(|observation| {
                        observation["amount_microtari"].as_u64().with_context(|| {
                            format!("submission {label} confirmed observation lacks amount")
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
                    .into_iter()
                    .try_fold(0u64, |total, amount| total.checked_add(amount))
                    .with_context(|| {
                        format!("submission {label} confirmed outgoing total overflows")
                    })?;
                if scenario != "S0" {
                    let reconciliation_outgoing =
                        reconciliation_outgoing_microtari(mode, scenario, confirmed_outgoing_total);
                    validate_balance_reconciliation(
                        metrics,
                        &label,
                        run["fee_microtari"].as_u64(),
                        reconciliation_outgoing,
                    )?;
                }
                let has_balance = metrics.contains_key("balance_reconciliation")
                    || metrics
                        .get("balance_delta_microtari")
                        .is_some_and(|value| !value.is_null())
                    || metrics
                        .get("extra")
                        .and_then(Value::as_object)
                        .is_some_and(|extra| extra.contains_key("balance_reconciliation"));
                let has_unavailable_reason = metrics
                    .get("balance_reconciliation_unavailable_reason")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| !reason.is_empty());
                if !has_balance && !has_unavailable_reason {
                    bail!(
                        "submission {label} must record final balance reconciliation or an explicit unavailable reason"
                    );
                }
            }
        }
    }
    Ok(())
}

fn reconciliation_outgoing_microtari(_mode: &str, scenario: &str, confirmed_total: u64) -> u64 {
    if scenario == "S1" { 0 } else { confirmed_total }
}

fn validate_balance_reconciliation(
    metrics: &serde_json::Map<String, Value>,
    label: &str,
    fee_microtari: Option<u64>,
    confirmed_outgoing_microtari: u64,
) -> anyhow::Result<()> {
    let Some(reconciliation) = metrics.get("balance_reconciliation") else {
        return Ok(());
    };
    let fee = fee_microtari.context("balance reconciliation requires a recorded fee")?;
    let outgoing = metrics
        .get("outgoing_microtari")
        .and_then(Value::as_u64)
        .context("balance reconciliation requires outgoing_microtari")?;
    if outgoing != confirmed_outgoing_microtari {
        bail!("{label} outgoing_microtari does not equal confirmed observation amounts");
    }
    let domain = reconciliation["balance_domain"]
        .as_str()
        .context("balance reconciliation requires an explicit balance_domain")?;
    let (before, after) = match domain {
        "available" => (
            metrics
                .get("balance_before_microtari")
                .and_then(Value::as_u64),
            metrics
                .get("balance_after_microtari")
                .and_then(Value::as_u64),
        ),
        "total" => {
            let before = metrics
                .get("balance_before")
                .and_then(|value| value.get("total"))
                .and_then(Value::as_u64)
                .context("balance reconciliation requires balance_before.total")?;
            let after = metrics
                .get("balance_after")
                .and_then(|value| value.get("total"))
                .and_then(Value::as_u64)
                .context("balance reconciliation requires balance_after.total")?;
            (Some(before), Some(after))
        }
        _ => bail!("{label} balance reconciliation has an unknown balance_domain"),
    };
    let before = before.context("balance reconciliation source before is unavailable")?;
    let after = after.context("balance reconciliation source after is unavailable")?;
    let deduction = outgoing
        .checked_add(fee)
        .context("balance reconciliation outgoing amount and fee overflow")?;
    let expected = before
        .checked_sub(deduction)
        .with_context(|| format!("{label} balance reconciliation underflows {domain} balance"))?;
    let delta = i128::from(expected) - i128::from(after);
    if reconciliation["expected_balance_microtari"] != json!(expected)
        || reconciliation["observed_balance_microtari"] != json!(after)
        || reconciliation["delta_microtari"] != json!(delta)
        || reconciliation["flagged"] != json!(delta != 0)
        || reconciliation["balance_domain"].as_str() != Some(domain)
    {
        bail!("{label} balance reconciliation arithmetic is inconsistent");
    }
    Ok(())
}

fn validate_s4_deadline_accounting(
    document: &Value,
    mode: &str,
    repetition: &Value,
    metrics: &Value,
    arms: &[Value],
    batch_summaries: &[Value],
) -> anyhow::Result<()> {
    let budget_ms = document["config"]["S4_T_budget_secs"]
        .as_u64()
        .context("submission config lacks S4_T_budget_secs")?
        .checked_mul(1_000)
        .context("submission S4 budget overflows milliseconds")?;
    let mut measured_total = 0u64;
    for ((arm, summary), configured_batch) in
        arms.iter().zip(batch_summaries).zip(REFERENCE_S4_RAMP)
    {
        let wall_ms = arm["wall_ms"]
            .as_u64()
            .with_context(|| format!("submission {mode}/S4 arm lacks wall_ms"))?;
        if wall_ms > budget_ms || summary["wall_ms"].as_u64() != Some(wall_ms) {
            bail!(
                "submission {mode}/S4 arm {configured_batch} exceeds its budget or disagrees with its batch summary"
            );
        }
        measured_total = measured_total
            .checked_add(wall_ms)
            .context("submission S4 measured wall total overflowed")?;
    }
    if repetition["wall_ms"].as_u64() != Some(measured_total) {
        bail!("submission {mode}/S4 repetition wall does not equal the five measured arms");
    }

    if mode == "new_wallet" {
        let quiescence = metrics["s4_deadline_quiescence"]
            .as_array()
            .context("submission new_wallet/S4 lacks deadline quiescence evidence")?;
        if quiescence.len() != REFERENCE_S4_RAMP.len() {
            bail!("submission new_wallet/S4 lacks one quiescence row per arm");
        }
        let mut actual_total = 0u64;
        let mut cleanup_total = 0u64;
        for ((row, arm), configured_batch) in quiescence.iter().zip(arms).zip(REFERENCE_S4_RAMP) {
            let arm_wall_ms = arm["wall_ms"].as_u64().expect("checked above");
            let actual_arm_wall_ms = row["actual_arm_wall_ms"]
                .as_u64()
                .context("submission new_wallet/S4 lacks actual arm wall")?;
            let cleanup_overrun_ms = row["cleanup_overrun_ms"]
                .as_u64()
                .context("submission new_wallet/S4 lacks cleanup overrun")?;
            if row["configured_batch"] != configured_batch
                || row["arm_wall_ms"].as_u64() != Some(arm_wall_ms)
                || arm_wall_ms.checked_add(cleanup_overrun_ms) != Some(actual_arm_wall_ms)
                || row["cleanup_excluded_from_measured_wall"] != true
                || row["tasks_quiesced_before_return"] != true
                || row["worker_reaped"] != true
                || row["unstructured_tasks_after_quiescence"] != 0
                || row["pending_tasks_at_deadline"]
                    .as_u64()
                    .is_none_or(|pending| pending > configured_batch)
                || row["post_deadline_drained_tasks"] != 0
                || row["post_deadline_drain_ms"]
                    .as_u64()
                    .is_none_or(|drain| drain > cleanup_overrun_ms)
                || row["quiesced_offset_ms"]
                    .as_u64()
                    .is_none_or(|offset| offset > actual_arm_wall_ms)
            {
                bail!(
                    "submission new_wallet/S4 arm {configured_batch} has inconsistent deadline or worker-quiescence accounting"
                );
            }
            actual_total = actual_total
                .checked_add(actual_arm_wall_ms)
                .context("submission new_wallet/S4 actual wall total overflowed")?;
            cleanup_total = cleanup_total
                .checked_add(cleanup_overrun_ms)
                .context("submission new_wallet/S4 cleanup total overflowed")?;
        }
        let end_to_end = metrics["s4_end_to_end_wall_ms"]
            .as_u64()
            .context("submission new_wallet/S4 lacks end-to-end wall")?;
        if metrics["s4_measured_wall_ms"].as_u64() != Some(measured_total)
            || metrics["s4_actual_scenario_wall_ms"].as_u64() != Some(actual_total)
            || metrics["s4_cleanup_overrun_ms"].as_u64() != Some(cleanup_total)
            || end_to_end < actual_total
            || metrics["s4_unattributed_orchestration_wall_ms"].as_u64()
                != Some(end_to_end - actual_total)
        {
            bail!("submission new_wallet/S4 aggregate deadline accounting is inconsistent");
        }
    } else {
        let measurements = metrics["s4_deadline_measurements"]
            .as_array()
            .with_context(|| format!("submission {mode}/S4 lacks deadline measurements"))?;
        let cleanups = metrics["s4_post_deadline_cleanups"]
            .as_array()
            .with_context(|| format!("submission {mode}/S4 lacks cleanup measurements"))?;
        if measurements.len() != REFERENCE_S4_RAMP.len()
            || cleanups.len() != REFERENCE_S4_RAMP.len()
        {
            bail!("submission {mode}/S4 lacks one deadline and cleanup row per arm");
        }
        let mut actual_return_total = 0u64;
        for (((measurement, cleanup), arm), configured_batch) in measurements
            .iter()
            .zip(cleanups)
            .zip(arms)
            .zip(REFERENCE_S4_RAMP)
        {
            let arm_wall_ms = arm["wall_ms"].as_u64().expect("checked above");
            let pending = measurement["pending_tasks_at_deadline"]
                .as_u64()
                .context("submission S4 deadline measurement lacks pending task count")?;
            let drained = cleanup["drained_tasks"]
                .as_u64()
                .context("submission S4 cleanup lacks drained task count")?;
            let forced = cleanup["forced_shutdown_tasks"]
                .as_u64()
                .context("submission S4 cleanup lacks forced task count")?;
            let abort_requested = cleanup["abort_requested_tasks"]
                .as_u64()
                .context("submission S4 cleanup lacks abort-request count")?;
            let completed_discarded = cleanup["completed_after_deadline_discarded_tasks"]
                .as_u64()
                .context("submission S4 cleanup lacks discarded-completion count")?;
            let unmatched = cleanup["unmatched_pending_attempts_after_drain"]
                .as_array()
                .context("submission S4 cleanup lacks unmatched attempt evidence")?;
            let actual_return = cleanup["actual_return_offset_ms"]
                .as_u64()
                .context("submission S4 cleanup lacks actual return offset")?;
            if measurement["configured_batch"] != configured_batch
                || measurement["deadline_offset_ms"].as_u64() != Some(budget_ms)
                || measurement["measured_wall_ms"].as_u64() != Some(arm_wall_ms)
                || pending > configured_batch
                || cleanup["configured_batch"] != configured_batch
                || cleanup["tasks_quiesced"] != true
                || cleanup["cleanup_grace_ms"] != 0
                || cleanup["cleanup_deadline_exhausted"] != false
                || cleanup["late_results_accepted"] != false
                || cleanup["post_deadline_join_errors"].as_array().is_none()
                || abort_requested != pending
                || drained.checked_add(forced) != Some(abort_requested)
                || completed_discarded > drained
                || unmatched.len() as u64 != pending
                || unmatched.iter().any(|attempt| {
                    attempt["terminal_outcome"] != "timed_out"
                        || attempt["submission_state"] != "ambiguous"
                })
                || actual_return < arm_wall_ms
            {
                bail!(
                    "submission {mode}/S4 arm {configured_batch} has inconsistent deadline or cleanup accounting"
                );
            }
            actual_return_total = actual_return_total
                .checked_add(actual_return)
                .context("submission S4 actual return total overflowed")?;
        }
        if metrics["s4_actual_return_wall_ms"]
            .as_u64()
            .is_none_or(|wall| wall < actual_return_total)
        {
            bail!("submission {mode}/S4 aggregate return wall is inconsistent");
        }
    }
    Ok(())
}

fn validate_s1_round_bindings(mode: &str, metrics: &Value) -> anyhow::Result<()> {
    let observations = metrics["transaction_observations"]
        .as_array()
        .with_context(|| format!("submission {mode}/S1 lacks transaction observations"))?;
    let batch_summaries = metrics["batch_summaries"]
        .as_array()
        .with_context(|| format!("submission {mode}/S1 lacks batch summaries"))?;
    if batch_summaries.len() != REFERENCE_S1_ROUNDS.len() {
        bail!("submission {mode}/S1 must contain one batch summary for every exact round");
    }

    for ((round_index, tx_count, outputs_per_tx, target_utxos_after), summary) in
        REFERENCE_S1_ROUNDS.into_iter().zip(batch_summaries)
    {
        let round_observations = observations
            .iter()
            .filter(|observation| observation["configured_batch"].as_u64() == Some(round_index))
            .collect::<Vec<_>>();
        if round_observations.len() as u64 != tx_count {
            bail!(
                "submission {mode}/S1 round {round_index} does not bind exactly {tx_count} observations"
            );
        }
        if round_observations.iter().any(|observation| {
            observation["terminal_outcome"] != "confirmed"
                || observation["input_count"] != 1
                || observation["payment_output_count"] != outputs_per_tx - 1
                || observation["change_output_count"] != 1
                || observation["total_output_count"] != outputs_per_tx
        }) {
            bail!(
                "submission {mode}/S1 round {round_index} does not prove the exact one-input split shape"
            );
        }
        let confirmed_fee = round_observations
            .iter()
            .map(|observation| {
                observation["fee_microtari"].as_u64().with_context(|| {
                    format!(
                        "submission {mode}/S1 round {round_index} confirmed observation lacks fee"
                    )
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .try_fold(0u64, |total, fee| total.checked_add(fee))
            .context("submission S1 round fee total overflows u64")?;

        let key = format!("round_{round_index}");
        let mut round_metrics = Vec::new();
        if let Some(value) = metrics.get(&key).filter(|value| !value.is_null()) {
            round_metrics.push(value);
        }
        if let Some(value) = metrics["extra"].get(&key).filter(|value| !value.is_null()) {
            round_metrics.push(value);
        }
        for rounds in [
            metrics["rounds"].as_array(),
            metrics["extra"]["rounds"].as_array(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(value) = rounds
                .iter()
                .find(|round| round["round_index"] == round_index)
            {
                round_metrics.push(value);
            }
        }
        if round_metrics.is_empty() {
            bail!("submission {mode}/S1 lacks measured metrics for round {round_index}");
        }
        let attempted = summary
            .get("attempted")
            .or_else(|| summary.get("attempted_batches"))
            .and_then(Value::as_u64);
        if summary["configured_batch"] != round_index
            || attempted != Some(tx_count)
            || summary
                .get("fee_microtari")
                .is_some_and(|fee| fee.as_u64() != Some(confirmed_fee))
        {
            bail!(
                "submission {mode}/S1 round {round_index} batch or fee accounting is inconsistent"
            );
        }
        let summary_wall = summary["wall_ms"]
            .as_u64()
            .with_context(|| format!("submission {mode}/S1 round {round_index} lacks wall time"))?;
        let mut found_fee = false;
        for measured in round_metrics {
            if measured
                .get("round_index")
                .is_some_and(|value| value.as_u64() != Some(round_index))
                || measured
                    .get("tx_count")
                    .is_some_and(|value| value.as_u64() != Some(tx_count))
                || measured
                    .get("outputs_per_tx")
                    .is_some_and(|value| value.as_u64() != Some(outputs_per_tx))
                || measured
                    .get("target_utxos_after")
                    .is_some_and(|value| value.as_u64() != Some(target_utxos_after))
                || measured
                    .get("failure_count")
                    .is_some_and(|value| value.as_u64() != Some(0))
                || measured
                    .get("wall_ms")
                    .is_some_and(|value| value.as_u64() != Some(summary_wall))
            {
                bail!(
                    "submission {mode}/S1 round {round_index} contains contradictory round metrics"
                );
            }
            if let Some(fee) = measured
                .get("total_fee_microtari")
                .or_else(|| measured.get("observed_fee_microtari"))
            {
                found_fee = true;
                if fee.as_u64() != Some(confirmed_fee) {
                    bail!(
                        "submission {mode}/S1 round {round_index} measured fee disagrees with confirmed observations"
                    );
                }
            }
        }
        if !found_fee {
            bail!("submission {mode}/S1 round {round_index} lacks a measured fee total");
        }
    }
    Ok(())
}

fn validate_paired_scan_targets(
    mode: &str,
    scenarios: &serde_json::Map<String, Value>,
    genesis_scenario: &str,
    birthday_scenario: &str,
    expected_checkpoint_prefix: &str,
) -> anyhow::Result<()> {
    let genesis = &scenarios[genesis_scenario];
    let birthday = &scenarios[birthday_scenario];
    if genesis["execution_status"] != birthday["execution_status"] {
        bail!(
            "submission {mode}/{genesis_scenario} and {birthday_scenario} must share one execution state"
        );
    }
    if genesis["execution_status"] != "completed" {
        return Ok(());
    }
    let genesis_runs = genesis["repetitions"].as_array().expect("schema checked");
    let birthday_runs = birthday["repetitions"].as_array().expect("schema checked");
    if genesis_runs.is_empty() || genesis_runs.len() != birthday_runs.len() {
        bail!(
            "submission {mode}/{genesis_scenario} and {birthday_scenario} must contain the same non-zero repetition count"
        );
    }
    let reference = &genesis_runs[0]["metrics"];
    let target_height = reference["H_tip_end"].as_u64().with_context(|| {
        format!("submission {mode}/{genesis_scenario} lacks its shared target height")
    })?;
    let target_hash = reference["H_tip_target_hash"].as_str().with_context(|| {
        format!("submission {mode}/{genesis_scenario} lacks its shared target hash")
    })?;
    let checkpoint = reference["scan_checkpoint"].as_str().with_context(|| {
        format!("submission {mode}/{genesis_scenario} lacks its scan checkpoint")
    })?;
    if !checkpoint.starts_with(expected_checkpoint_prefix) {
        bail!("submission {mode}/{genesis_scenario} has a checkpoint outside the expected pair");
    }
    for (scenario, runs) in [
        (genesis_scenario, genesis_runs),
        (birthday_scenario, birthday_runs),
    ] {
        for (index, run) in runs.iter().enumerate() {
            let metrics = &run["metrics"];
            if metrics["H_tip_end"].as_u64() != Some(target_height)
                || metrics["H_tip_target_hash"].as_str() != Some(target_hash)
                || metrics["scan_checkpoint"].as_str() != Some(checkpoint)
            {
                bail!(
                    "submission {mode}/{scenario} repetition {} is not bound to the pair's immutable target and checkpoint",
                    index + 1
                );
            }
            for field in [
                "expected_outputs",
                "expected_available_microtari",
                "expected_history_transactions",
                "expected_history_tx_ids",
                "expected_output_commitments",
            ] {
                if metrics[field] != reference[field] {
                    bail!(
                        "submission {mode}/{genesis_scenario} and {birthday_scenario} differ in {field}"
                    );
                }
            }
        }
    }
    Ok(())
}

fn scan_target_binding_valid(mode: &str, metrics: &Value) -> bool {
    let exact_target = metrics["max_height"] == metrics["H_tip_end"]
        && metrics["H_scan_cursor_hash"] == metrics["H_tip_target_hash"];
    let anchored_mode1_overshoot = mode == "old_wallet"
        && metrics["scan_target_overshoot_allowed"] == true
        && metrics["max_height"]
            .as_u64()
            .zip(metrics["H_tip_end"].as_u64())
            .is_some_and(|(cursor, target)| cursor >= target)
        && metrics["H_scan_target_cursor_hash"] == metrics["H_tip_target_hash"];
    exact_target || anchored_mode1_overshoot
}

fn validate_scenario_specific_metrics(document: &Value) -> anyhow::Result<()> {
    let mut recipient_set: Option<Vec<Value>> = None;
    let mut b0_target: Option<(u64, &str)> = None;
    for (mode, scenarios) in mode_scenarios(document)? {
        if scenarios["B0"]["repetitions"]
            .as_array()
            .is_none_or(|runs| runs.len() != 1)
        {
            bail!("submission B0 must contain exactly one repetition for {mode}");
        }
        let b0_run = &scenarios["B0"]["repetitions"][0];
        let b0 = &b0_run["metrics"];
        if scenarios["B0"]["execution_status"] != "completed"
            || b0_run["execution_status"] != "completed"
            || scenarios["B0"]["outcome_status"] != b0_run["outcome_status"]
        {
            bail!("submission B0 cell/repetition status mismatch for {mode}");
        }
        let target_height = b0["H_tip_end"]
            .as_u64()
            .with_context(|| format!("submission B0 target height missing for {mode}"))?;
        let target_hash = b0["H_tip_target_hash"]
            .as_str()
            .with_context(|| format!("submission B0 target hash missing for {mode}"))?;
        if let Some(expected) = b0_target {
            if expected != (target_height, target_hash) {
                bail!("submission B0 modes do not share one target height and hash");
            }
        } else {
            b0_target = Some((target_height, target_hash));
        }
        if b0["birthday"] != 0
            || b0["expected_outputs"] != 0
            || b0["expected_available_microtari"] != 0
            || b0["expected_history_transactions"] != 0
            || b0["detected_outputs"] != 0
            || b0["spendable_outputs"] != 0
            || b0["available_microtari"] != 0
            || b0["history_transactions"] != 0
            || b0["outputs_match_expected"] != true
            || b0["balance_matches_expected"] != true
            || b0["history_count_matches_expected"] != true
            || b0["tip_lag_tolerance_blocks"] != 0
            || b0["H_tip_completion"].as_u64().is_none()
            || b0["H_tip_completion_hash"].as_str().is_none()
        {
            bail!("submission B0 empty-wallet evidence failed for {mode}");
        }
        let funding = &document["funding"][mode];
        if funding["height"]
            .as_u64()
            .is_none_or(|height| height <= target_height)
            || funding["tip_height_at_broadcast"]
                .as_u64()
                .is_none_or(|height| height <= target_height)
        {
            bail!("submission funding for {mode} did not occur after the shared B0 target");
        }
        for key in [
            "T_scan_ms",
            "blocks_per_sec",
            "H_tip_start",
            "H_tip_end",
            "peak_rss_bytes",
            "peak_cpu_percent",
            "scan_invocations",
            "max_height",
            "tip_lag_blocks",
            "scan_no_progress_attempts",
        ] {
            if !b0[key].is_number() {
                bail!("submission B0 metric {key} missing for {mode}");
            }
        }
        if b0["H_scan_cursor_hash"].as_str().is_none()
            || b0["scan_reached_tip"].as_bool().is_none()
            || b0["scan_stopped_without_progress"].as_bool().is_none()
            || b0["scan_timed_out"].as_bool().is_none()
            || b0["scan_invocations"] != 1
            || b0_run["wall_ms"] != b0["T_scan_ms"]
            || b0_run["fee_microtari"] != 0
        {
            bail!("submission B0 measured evidence is incomplete for {mode}");
        }
        match b0_run["outcome_status"].as_str() {
            Some("success") => {
                if b0_run["success_count"] != 1
                    || b0_run["failure_count"] != 0
                    || !b0_run["error"].is_null()
                    || b0["max_height"] != b0["H_tip_end"]
                    || b0["scan_reached_tip"] != true
                    || b0["tip_lag_blocks"] != 0
                    || b0["H_scan_cursor_hash"].as_str() != Some(target_hash)
                    || b0["H_scan_target_cursor_hash"].as_str() != Some(target_hash)
                    || b0["scan_overshoot_blocks"] != 0
                    || b0["scan_target_overshoot_allowed"] != false
                    || b0["scan_stopped_without_progress"] != false
                    || b0["scan_no_progress_attempts"] != 0
                    || b0["scan_timed_out"] != false
                    || !b0["scan_timeout_ms"].is_null()
                    || !b0["scan_error"].is_null()
                    || !b0["scan_stop_reason"].is_null()
                {
                    bail!("submission B0 exact-tip success contract failed for {mode}");
                }
            }
            Some("failure") => {
                let max_height = b0["max_height"].as_u64().expect("checked as numeric above");
                let lag = b0["tip_lag_blocks"]
                    .as_u64()
                    .expect("checked as numeric above");
                let timed_out = b0["scan_timed_out"]
                    .as_bool()
                    .expect("checked as boolean above");
                let stop_reason = b0["scan_stop_reason"].as_str();
                let stopped_without_progress = b0["scan_stopped_without_progress"]
                    .as_bool()
                    .expect("checked as boolean above");
                let no_progress_attempts = b0["scan_no_progress_attempts"]
                    .as_u64()
                    .expect("checked as numeric above");
                if b0_run["success_count"] != 0
                    || b0_run["failure_count"] != 1
                    || b0_run["error"].as_str().is_none_or(str::is_empty)
                    || b0["scan_reached_tip"] == true
                    || mode == "old_wallet"
                    || max_height >= target_height
                    || lag == 0
                    || lag != target_height - max_height
                    || !b0["H_scan_target_cursor_hash"].is_null()
                    || b0["scan_overshoot_blocks"] != 0
                    || b0["scan_target_overshoot_allowed"] != false
                    || (timed_out && stop_reason != Some("scan_timeout"))
                    || (timed_out
                        && (b0["scan_timeout_ms"].as_u64().is_none()
                            || b0["scan_error"].as_str().is_none()))
                    || (!timed_out
                        && (stop_reason != Some("scanner_returned_below_fixed_target")
                            || b0["scan_last_more_blocks"].as_bool().is_none()
                            || no_progress_attempts != u64::from(stopped_without_progress)
                            || !b0["scan_timeout_ms"].is_null()
                            || !b0["scan_error"].is_null()))
                {
                    bail!("submission B0 wallet-failure evidence failed for {mode}");
                }
            }
            _ => bail!("submission B0 did not complete for {mode}"),
        }
        for scenario in ["S1", "S4", "S5"] {
            if scenarios[scenario]["execution_status"] != "completed" {
                continue;
            }
            let metrics = &scenarios[scenario]["repetitions"][0]["metrics"];
            let start_tip = metrics["scenario_tip_start_height"].as_u64();
            let end_tip = metrics["scenario_tip_end_height"].as_u64();
            if start_tip.is_none() || end_tip.is_none() || end_tip < start_tip {
                bail!("submission {mode}/{scenario} is missing coherent scenario tip anchors");
            }
            let before = metrics
                .get("balance_before")
                .or_else(|| metrics["extra"].get("balance_before"));
            let after = metrics
                .get("balance_after")
                .or_else(|| metrics["extra"].get("balance_after"));
            if before.is_none_or(Value::is_null) || after.is_none_or(Value::is_null) {
                bail!("submission {mode}/{scenario} is missing balance components");
            }
            let required_balance_keys: &[&str] = if mode == "old_wallet" {
                &[
                    "available",
                    "pending_incoming",
                    "pending_outgoing",
                    "timelocked",
                ]
            } else {
                &["total", "available", "locked", "unconfirmed", "immature"]
            };
            for balance in [
                before.expect("checked above"),
                after.expect("checked above"),
            ] {
                if required_balance_keys.iter().any(|key| {
                    balance
                        .get(*key)
                        .is_none_or(|value| !is_amount_value(value))
                }) {
                    bail!("submission {mode}/{scenario} balance components are incomplete");
                }
            }
        }

        let s1_metrics = &scenarios["S1"]["repetitions"][0]["metrics"];
        if scenarios["S1"]["outcome_status"] == "success" {
            for (round_index, tx_count, outputs_per_tx, target) in REFERENCE_S1_ROUNDS {
                let key = format!("round_{round_index}");
                let direct = s1_metrics.get(&key).filter(|value| !value.is_null());
                let nested = s1_metrics["extra"]
                    .get(&key)
                    .filter(|value| !value.is_null());
                let array = s1_metrics["rounds"].as_array().and_then(|rounds| {
                    rounds
                        .iter()
                        .find(|round| round["round_index"] == round_index)
                });
                let plan = s1_metrics["extra"]["rounds"].as_array().and_then(|rounds| {
                    rounds
                        .iter()
                        .find(|round| round["round_index"] == round_index)
                });
                let observed = direct.or(nested).or(array).with_context(|| {
                    format!("submission {mode}/S1 missing round {round_index} metrics")
                })?;
                let shape = array
                    .or(plan)
                    .or(direct)
                    .or(nested)
                    .expect("observed above");
                if shape["tx_count"] != tx_count
                    || shape["outputs_per_tx"] != outputs_per_tx
                    || shape["target_utxos_after"] != target
                    || observed["wall_ms"].as_u64().is_none()
                    || observed["failure_count"] != 0
                    || observed["fee_only_balance_delta_ok"] != true
                {
                    bail!("submission {mode}/S1 round {round_index} contract failed");
                }
            }
            validate_transaction_observations(mode, "S1", s1_metrics)?;
            if s1_metrics["transaction_observations"]
                .as_array()
                .is_none_or(|observations| observations.len() != 127)
            {
                bail!(
                    "submission {mode}/S1 must contain one observation for each of 127 transactions"
                );
            }
            let s1_observations = s1_metrics["transaction_observations"]
                .as_array()
                .expect("checked above");
            let two_output = s1_observations
                .iter()
                .filter(|observation| {
                    observation["input_count"] == 1 && observation["total_output_count"] == 2
                })
                .count();
            let eight_output = s1_observations
                .iter()
                .filter(|observation| {
                    observation["input_count"] == 1 && observation["total_output_count"] == 8
                })
                .count();
            if two_output != 63 || eight_output != 64 {
                bail!(
                    "submission {mode}/S1 does not prove 63 one-to-two and 64 one-to-eight transactions"
                );
            }
            validate_s1_round_bindings(mode, s1_metrics)?;
        } else {
            validate_transaction_observations(mode, "S1", s1_metrics)?;
        }

        if scenarios["S4"]["execution_status"] == "completed" {
            let s4_repetition = &scenarios["S4"]["repetitions"][0];
            let s4 = &s4_repetition["metrics"];
            let batch_summaries = s4["batch_summaries"].as_array();
            let arms = s4["s4_arms"].as_array();
            if batch_summaries.is_none()
                || arms.is_none()
                || s4["double_selection_rejections"].as_u64().is_none()
            {
                bail!("submission {mode}/S4 is missing concurrency batch metrics");
            }
            let batch_summaries = batch_summaries.expect("checked above");
            if batch_summaries.len() != REFERENCE_S4_RAMP.len()
                || batch_summaries
                    .iter()
                    .zip(REFERENCE_S4_RAMP)
                    .any(|(summary, expected)| {
                        summary["configured_batch"].as_u64() != Some(expected)
                            || summary
                                .get("attempted")
                                .or_else(|| summary.get("attempted_batches"))
                                .and_then(Value::as_u64)
                                != Some(expected)
                            || summary["wall_ms"].as_u64().is_none()
                    })
            {
                bail!("submission {mode}/S4 does not contain the exact 8,16,32,64,128 arms");
            }
            validate_transaction_observations(mode, "S4", s4)?;
            if s4["transaction_observations"]
                .as_array()
                .is_none_or(|observations| observations.len() != 248)
            {
                bail!("submission {mode}/S4 must contain one observation for each of 248 attempts");
            }
            let observations = s4["transaction_observations"]
                .as_array()
                .expect("checked above");
            for configured_batch in REFERENCE_S4_RAMP {
                if observations
                    .iter()
                    .filter(|observation| {
                        observation["configured_batch"].as_u64() == Some(configured_batch)
                    })
                    .count()
                    != configured_batch as usize
                {
                    bail!(
                        "submission {mode}/S4 observations are not bound to configured arm {configured_batch}"
                    );
                }
            }
            let arms = arms.expect("checked above");
            if arms.len() != REFERENCE_S4_RAMP.len() {
                bail!("submission {mode}/S4 is missing per-arm metrics");
            }
            for (arm, configured_batch) in arms.iter().zip(REFERENCE_S4_RAMP) {
                let attempted = arm["attempted"].as_u64().unwrap_or_default();
                let accepted = arm["accepted"].as_u64().unwrap_or_default();
                let confirmed = arm["confirmed"].as_u64().unwrap_or_default();
                let terminal = confirmed
                    + arm["rejected"].as_u64().unwrap_or_default()
                    + arm["stalled"].as_u64().unwrap_or_default()
                    + arm["timed_out"].as_u64().unwrap_or_default();
                if arm["configured_batch"] != configured_batch
                    || attempted != configured_batch
                    || terminal != attempted
                    || accepted > attempted
                    || arm["recipients_are_distinct"] != true
                    || arm["distinct_recipient_count"] != configured_batch
                {
                    bail!(
                        "submission {mode}/S4 arm {configured_batch} has inconsistent counts or recipients"
                    );
                }
                let expected_success = confirmed as f64 / attempted as f64;
                let expected_accept = accepted as f64 / attempted as f64;
                if (arm["confirmed_success_rate"].as_f64().unwrap_or(-1.0) - expected_success).abs()
                    > f64::EPSILON
                    || (arm["api_acceptance_rate"].as_f64().unwrap_or(-1.0) - expected_accept).abs()
                        > f64::EPSILON
                {
                    bail!("submission {mode}/S4 arm {configured_batch} has inconsistent rates");
                }
                let mut genuine_offsets = observations
                    .iter()
                    .filter(|observation| {
                        observation["configured_batch"].as_u64() == Some(configured_batch)
                            && observation["construction_timing_origin"]
                                .as_str()
                                .is_some_and(|origin| match mode {
                                    "new_wallet" => origin.starts_with("library_build_and_sign"),
                                    "payment_processor" => {
                                        origin
                                            == "payment_processor_db_unsigned_tx_first_observation"
                                    }
                                    _ => false,
                                })
                    })
                    .filter_map(|observation| {
                        observation["construction_complete_offset_ms"].as_u64()
                    })
                    .collect::<Vec<_>>();
                genuine_offsets.sort_unstable();
                let expected_gap = (genuine_offsets.len() >= 2).then(|| {
                    genuine_offsets
                        .windows(2)
                        .map(|pair| pair[1].saturating_sub(pair[0]))
                        .max()
                        .unwrap_or_default()
                });
                if let Some(expected_gap) = expected_gap {
                    if arm["max_serialization_gap_ms"].as_u64() != Some(expected_gap) {
                        bail!(
                            "submission {mode}/S4 arm {configured_batch} has invalid serialization evidence"
                        );
                    }
                } else if !arm["max_serialization_gap_ms"].is_null()
                    || arm["serialization_timing_reason"]
                        .as_str()
                        .unwrap_or_default()
                        .is_empty()
                {
                    bail!(
                        "submission {mode}/S4 arm {configured_batch} lacks an honest unavailable timing reason"
                    );
                }
            }
            validate_s4_deadline_accounting(
                document,
                mode,
                s4_repetition,
                s4,
                arms,
                batch_summaries,
            )?;
        }

        if scenarios["S5"]["execution_status"] == "completed" {
            let s5 = &scenarios["S5"]["repetitions"][0]["metrics"];
            let set = s5
                .get("recipient_set")
                .or_else(|| s5["extra"].get("recipient_set"))
                .and_then(Value::as_array)
                .with_context(|| format!("submission {mode}/S5 missing recipient_set"))?;
            if set.len() != 100
                || recipient_set
                    .as_ref()
                    .is_some_and(|expected| expected != set)
            {
                bail!("submission S5 recipient set is not the same 100 addresses for every mode");
            }
            let unique = set
                .iter()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>();
            if unique.len() != 100 {
                bail!("submission S5 recipient set must contain 100 unique addresses");
            }
            recipient_set = Some(set.clone());
            if s5
                .get("unspent_before")
                .or_else(|| s5["extra"].get("unspent_before"))
                .is_none()
                || s5
                    .get("balance_before_microtari")
                    .or_else(|| s5["extra"].get("balance_before_microtari"))
                    .is_none()
            {
                bail!("submission {mode}/S5 missing disclosed post-S4 starting state");
            }
            validate_transaction_observations(mode, "S5", s5)?;
            validate_s5_observation_amounts(document, mode, s5)?;
            for (repetition, run) in scenarios["S5"]["repetitions"]
                .as_array()
                .expect("schema checked")
                .iter()
                .enumerate()
            {
                let metrics = &run["metrics"];
                let Some(observations) = metrics["transaction_observations"].as_array() else {
                    continue;
                };
                let expected = set
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<BTreeSet<_>>();
                let mut observed = BTreeSet::new();
                for observation in observations {
                    let recipients = observation["recipients"].as_array().with_context(|| {
                        format!(
                            "submission {mode}/S5 repetition {} observation lacks recipients",
                            repetition + 1
                        )
                    })?;
                    if recipients.is_empty()
                        || recipients.iter().any(|recipient| {
                            recipient
                                .as_str()
                                .is_none_or(|recipient| !expected.contains(recipient))
                        })
                    {
                        bail!(
                            "submission {mode}/S5 repetition {} contains an unbound recipient",
                            repetition + 1
                        );
                    }
                    let distinct = recipients
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<BTreeSet<_>>();
                    if distinct.len() != recipients.len() {
                        bail!(
                            "submission {mode}/S5 repetition {} contains duplicate recipients",
                            repetition + 1
                        );
                    }
                    for recipient in recipients.iter().filter_map(Value::as_str) {
                        if !observed.insert(recipient) {
                            bail!(
                                "submission {mode}/S5 repetition {} repeats a recipient across observations",
                                repetition + 1
                            );
                        }
                    }
                }
                if scenarios["S5"]["outcome_status"] == "success" && observed != expected {
                    bail!(
                        "submission {mode}/S5 repetition {} does not cover exactly the declared recipient set",
                        repetition + 1
                    );
                }
            }
            let expected_s5_transactions = if mode == "payment_processor" { 10 } else { 100 };
            if s5["transaction_observations"]
                .as_array()
                .is_none_or(|observations| observations.len() != expected_s5_transactions)
            {
                bail!("submission {mode}/S5 has the wrong transaction observation count");
            }
            let expected_payment_outputs = if mode == "payment_processor" { 10 } else { 1 };
            if s5["transaction_observations"]
                .as_array()
                .expect("checked above")
                .iter()
                .filter(|observation| observation["terminal_outcome"] == "confirmed")
                .any(|observation| {
                    let total = observation["total_output_count"].as_u64();
                    let payments = observation["payment_output_count"].as_u64();
                    let change = observation["change_output_count"].as_u64();
                    observation["input_count"] != 1
                        || observation["payment_output_count"] != expected_payment_outputs
                        || total.is_none()
                        || change.is_none()
                        || change.is_some_and(|count| count > 1)
                        || total
                            != payments
                                .zip(change)
                                .map(|(payments, change)| payments + change)
                })
            {
                bail!(
                    "submission {mode}/S5 confirmed transaction has the wrong input/payment-output shape"
                );
            }
        }

        for scenario in ["S2", "S3", "S6", "S7"] {
            let cell = &scenarios[scenario];
            if cell["execution_status"] != "completed" {
                continue;
            }
            for run in cell["repetitions"].as_array().expect("schema checked") {
                let metrics = &run["metrics"];
                if run["outcome_status"] != "success" {
                    if metrics["H_tip_end"].as_u64().is_none()
                        || metrics["H_tip_target_hash"].as_str().is_none()
                        || metrics["max_height"].as_u64().is_none()
                        || metrics["scan_invocations"].as_u64().is_none()
                        || metrics["scan_timed_out"].as_bool().is_none()
                        || metrics["peak_rss_bytes"].as_u64().is_none()
                        || metrics["peak_cpu_percent"].as_f64().is_none()
                        || (metrics["scan_timed_out"] == true
                            && (metrics["scan_timeout_ms"].as_u64().is_none()
                                || metrics["scan_error"].as_str().is_none()))
                    {
                        bail!(
                            "submission {mode}/{scenario} failed scan is missing target, final progress, invocation, timeout, or resource evidence"
                        );
                    }
                    continue;
                }
                if metrics["birthday"].as_u64()
                    != Some(if matches!(scenario, "S2" | "S6") {
                        0
                    } else {
                        profile_funding_birthday(document, mode)?
                    })
                    || !scan_target_binding_valid(mode, metrics)
                    || metrics["tip_lag_tolerance_blocks"] != 0
                    || metrics["history_matches_expected"] != true
                    || metrics["history_identities_match_expected"] != true
                    || metrics["H_tip_target_hash"].as_str().is_none()
                    || metrics["scan_invocations"].as_u64().is_none()
                    || metrics["H_tip_completion"].as_u64().is_none()
                    || metrics["H_tip_completion_hash"].as_str().is_none()
                    || metrics["expected_history_tx_ids"].as_array().is_none()
                    || metrics["expected_output_commitments"].as_object().is_none()
                    || metrics["missing_history_tx_ids"] != json!([])
                    || metrics["missing_history_output_tx_ids"] != json!([])
                    || metrics["resource_sampling_window"] != "scan_wall_window"
                    || metrics["resource_sampling_process"].as_str().is_none()
                {
                    bail!("submission {mode}/{scenario} scan contract failed");
                }
                let (birthday_height, birthday_start_height, birthday_offset_days) =
                    if matches!(scenario, "S2" | "S6") {
                        (0, 0, 0)
                    } else {
                        (
                            document["funding"][mode]["birthday_height"]
                                .as_u64()
                                .context("current funding birthday_height missing")?,
                            document["funding"][mode]["birthday_start_height"]
                                .as_u64()
                                .context("current funding birthday_start_height missing")?,
                            document["funding"][mode]["birthday_offset_days"]
                                .as_u64()
                                .context("current funding birthday_offset_days missing")?,
                        )
                    };
                let max_height = metrics["max_height"]
                    .as_u64()
                    .context("successful scan max_height missing")?;
                if metrics["H_birth"] != birthday_height
                    || metrics["birthday_height"] != birthday_height
                    || metrics["birthday_start_height"] != birthday_start_height
                    || metrics["birthday_offset_days"] != birthday_offset_days
                    || metrics["blocks_scanned"] != max_height.saturating_sub(birthday_height)
                    || metrics["effective_blocks_scanned"]
                        != max_height.saturating_sub(birthday_start_height)
                    || metrics["blocks_per_sec"].as_f64().is_none()
                    || metrics["effective_blocks_per_sec"].as_f64().is_none()
                {
                    bail!(
                        "submission {mode}/{scenario} birthday scan accounting does not match H_tip_end and the stock offset"
                    );
                }
                if (mode == "old_wallet"
                    && metrics["expected_history_tx_ids"]
                        .as_array()
                        .is_none_or(Vec::is_empty))
                    || (mode != "old_wallet"
                        && metrics["expected_output_commitments"]
                            .as_object()
                            .is_none_or(serde_json::Map::is_empty))
                {
                    bail!("submission {mode}/{scenario} scan history expectation is empty");
                }
                validate_scan_history_evidence(document, mode, scenario, metrics)?;
            }
        }
        validate_paired_scan_targets(mode, scenarios, "S2", "S3", "post_s1")?;
        validate_paired_scan_targets(mode, scenarios, "S6", "S7", "post_s5")?;
    }
    Ok(())
}

fn validate_scan_history_evidence(
    document: &Value,
    mode: &str,
    scan_scenario: &str,
    metrics: &Value,
) -> anyhow::Result<()> {
    let source_scenarios: &[&str] = if matches!(scan_scenario, "S2" | "S3") {
        &["S1"]
    } else {
        &["S1", "S4", "S5"]
    };
    if mode == "old_wallet" {
        let expected = document["chain_verification"]["verified_transactions"]
            .as_array()
            .expect("schema checked")
            .iter()
            .filter(|tx| {
                tx["mode"] == mode
                    && tx["confirmed"] == true
                    && tx["scenario"]
                        .as_str()
                        .is_some_and(|scenario| source_scenarios.contains(&scenario))
            })
            .filter_map(|tx| tx["tx_id"].as_str().map(ToString::to_string))
            .collect::<BTreeSet<_>>();
        let reported = json_string_set(&metrics["expected_history_tx_ids"])?;
        let recovered = json_string_set(&metrics["recovered_history_tx_ids"])?;
        let missing = expected
            .difference(&recovered)
            .cloned()
            .collect::<BTreeSet<_>>();
        if reported != expected || json_string_set(&metrics["missing_history_tx_ids"])? != missing {
            bail!("submission {mode}/{scan_scenario} history transaction evidence is inconsistent");
        }
    } else {
        let expected = expected_observation_commitments(document, mode, source_scenarios)?;
        let reported = json_commitment_map(&metrics["expected_output_commitments"])?;
        let recovered = json_string_set(&metrics["recovered_output_commitments"])?;
        let missing = expected
            .iter()
            .filter(|(_, commitments)| commitments.is_disjoint(&recovered))
            .map(|(tx_id, _)| tx_id.clone())
            .collect::<BTreeSet<_>>();
        if reported != expected
            || json_string_set(&metrics["missing_history_output_tx_ids"])? != missing
        {
            bail!("submission {mode}/{scan_scenario} history commitment evidence is inconsistent");
        }
    }
    Ok(())
}

fn expected_observation_commitments(
    document: &Value,
    mode: &str,
    scenarios: &[&str],
) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
    let mut expected = BTreeMap::<String, BTreeSet<String>>::new();
    for scenario in scenarios {
        let observations = document["modes"][mode]["scenarios"][scenario]["repetitions"][0]
            ["metrics"]["transaction_observations"]
            .as_array()
            .with_context(|| format!("submission {mode}/{scenario} observations missing"))?;
        for observation in observations
            .iter()
            .filter(|observation| observation["terminal_outcome"] == "confirmed")
        {
            let tx_id = observation["transaction_id"]
                .as_str()
                .with_context(|| format!("submission {mode}/{scenario} confirmed tx id missing"))?;
            expected
                .entry(tx_id.to_string())
                .or_default()
                .extend(json_string_set(&observation["output_commitments"])?);
        }
    }
    Ok(expected)
}

fn json_string_set(value: &Value) -> anyhow::Result<BTreeSet<String>> {
    value
        .as_array()
        .context("expected an array of strings")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .context("expected a string")
        })
        .collect()
}

fn json_commitment_map(value: &Value) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
    value
        .as_object()
        .context("expected a transaction commitment map")?
        .iter()
        .map(|(tx_id, commitments)| Ok((tx_id.clone(), json_string_set(commitments)?)))
        .collect()
}

fn is_amount_value(value: &Value) -> bool {
    value.as_u64().is_some()
        || value.as_str().is_some_and(|value| {
            value.parse::<u64>().is_ok() || crate::config::parse_amount(value).is_ok()
        })
        || value.as_object().is_some_and(|value| {
            value.get("value").and_then(Value::as_u64).is_some()
                || value.get("microtari").and_then(Value::as_u64).is_some()
        })
}

fn profile_funding_birthday(document: &Value, mode: &str) -> anyhow::Result<u64> {
    document["funding"][mode]["birthday"]
        .as_u64()
        .with_context(|| format!("funding birthday missing for {mode}"))
}

fn validate_s1_preflight_failures(
    mode: &str,
    scenario: &str,
    metrics: &Value,
) -> anyhow::Result<usize> {
    let Some(value) = metrics
        .get("preflight_failures")
        .or_else(|| metrics["extra"].get("preflight_failures"))
    else {
        return Ok(0);
    };
    if scenario != "S1" {
        bail!("submission {mode}/{scenario} contains an S1-only preflight failure");
    }
    let failures = value
        .as_array()
        .with_context(|| format!("submission {mode}/S1 preflight_failures must be an array"))?;
    if failures.len() != 1 {
        bail!("submission {mode}/S1 must stop after its first preflight failure");
    }
    let failure = &failures[0];
    let round = failure["round_index"]
        .as_u64()
        .filter(|round| (1..=7).contains(round))
        .with_context(|| format!("submission {mode}/S1 preflight failure has an invalid round"))?;
    if failure["attempted_index"]
        .as_u64()
        .is_some_and(|attempt| attempt == 0)
        || failure["submission_started"] != false
        || !matches!(
            failure["stage"].as_str(),
            Some(
                "read_spendable_inputs"
                    | "validate_spendable_input_count"
                    | "select_spendable_input_floor"
                    | "plan_transaction"
            )
        )
        || failure["error"].as_str().is_none_or(str::is_empty)
    {
        bail!("submission {mode}/S1 round {round} has malformed preflight failure evidence");
    }
    Ok(failures.len())
}

fn validate_s1_constructed_output_shapes(
    mode: &str,
    scenario: &str,
    metrics: &Value,
    observations: &[Value],
) -> anyhow::Result<()> {
    if scenario != "S1" || !matches!(mode, "old_wallet" | "new_wallet") {
        return Ok(());
    }
    let transaction_ids = observations
        .iter()
        .filter_map(|observation| observation["transaction_id"].as_str())
        .filter(|tx_id| !tx_id.is_empty())
        .collect::<BTreeSet<_>>();
    if transaction_ids.is_empty() {
        if metrics["constructed_output_shapes_by_tx_id"]
            .as_object()
            .is_some_and(|shapes| !shapes.is_empty())
        {
            bail!(
                "submission {mode}/{scenario} contains constructed output shape evidence without a matching transaction observation"
            );
        }
        return Ok(());
    }
    let constructed_shapes = metrics["constructed_output_shapes_by_tx_id"]
        .as_object()
        .with_context(|| {
            format!("submission {mode}/{scenario} lacks constructed output shapes by transaction")
        })?;
    for tx_id in &transaction_ids {
        let constructed = constructed_shapes.get(*tx_id).with_context(|| {
            format!("submission {mode}/{scenario} lacks constructed output shape for {tx_id}")
        })?;
        let payment = constructed["payment_output_count"]
            .as_u64()
            .with_context(|| {
                format!(
                    "submission {mode}/{scenario} constructed output shape for {tx_id} lacks payment count"
                )
            })?;
        let change = constructed["change_output_count"]
            .as_u64()
            .with_context(|| {
                format!(
                    "submission {mode}/{scenario} constructed output shape for {tx_id} lacks change count"
                )
            })?;
        let total = constructed["total_output_count"]
            .as_u64()
            .with_context(|| {
                format!(
                    "submission {mode}/{scenario} constructed output shape for {tx_id} lacks total count"
                )
            })?;
        if change != 1 || payment.checked_add(change) != Some(total) {
            bail!(
                "submission {mode}/{scenario} constructed output shape for {tx_id} is not N-1 payments plus one change"
            );
        }
        for observation in observations
            .iter()
            .filter(|observation| observation["transaction_id"].as_str() == Some(*tx_id))
        {
            let observed_payment = observation["payment_output_count"].as_u64();
            let observed_change = observation["change_output_count"].as_u64();
            let observed_total = observation["total_output_count"].as_u64();
            let has_observed_shape =
                observed_payment.is_some() || observed_change.is_some() || observed_total.is_some();
            if (observation["terminal_outcome"] == "confirmed" || has_observed_shape)
                && (observed_payment, observed_change, observed_total)
                    != (Some(payment), Some(change), Some(total))
            {
                bail!(
                    "submission {mode}/{scenario} observation shape for {tx_id} does not match its constructed output roles"
                );
            }
        }
    }
    if constructed_shapes
        .keys()
        .any(|tx_id| !transaction_ids.contains(tx_id.as_str()))
    {
        bail!(
            "submission {mode}/{scenario} contains constructed output shape evidence without a matching transaction observation"
        );
    }
    Ok(())
}

fn validate_s1_constructed_amounts(
    mode: &str,
    scenario: &str,
    metrics: &Value,
    observations: &[Value],
) -> anyhow::Result<()> {
    if scenario != "S1" || mode != "new_wallet" {
        return Ok(());
    }
    let transaction_ids = observations
        .iter()
        .filter_map(|observation| observation["transaction_id"].as_str())
        .filter(|tx_id| !tx_id.is_empty())
        .collect::<BTreeSet<_>>();
    if transaction_ids.is_empty() {
        if metrics["constructed_amounts_microtari_by_tx_id"]
            .as_object()
            .is_some_and(|amounts| !amounts.is_empty())
        {
            bail!(
                "submission new_wallet/S1 contains constructed amount evidence without a matching transaction observation"
            );
        }
        return Ok(());
    }
    let constructed_amounts = metrics["constructed_amounts_microtari_by_tx_id"]
        .as_object()
        .context("submission new_wallet/S1 lacks constructed amounts by transaction")?;
    for tx_id in &transaction_ids {
        let amount = constructed_amounts
            .get(*tx_id)
            .and_then(Value::as_u64)
            .filter(|amount| *amount > 0)
            .with_context(|| {
                format!("submission new_wallet/S1 lacks a positive constructed amount for {tx_id}")
            })?;
        for observation in observations
            .iter()
            .filter(|observation| observation["transaction_id"].as_str() == Some(*tx_id))
        {
            if observation["amount_microtari"].as_u64() != Some(amount) {
                bail!(
                    "submission new_wallet/S1 observation amount for {tx_id} does not match its constructed amount"
                );
            }
        }
    }
    if constructed_amounts
        .keys()
        .any(|tx_id| !transaction_ids.contains(tx_id.as_str()))
    {
        bail!(
            "submission new_wallet/S1 contains constructed amount evidence without a matching transaction observation"
        );
    }
    if let Some(verified) = metrics["observed_transactions"].as_array() {
        for transaction in verified {
            let tx_id = transaction["tx_id"]
                .as_str()
                .context("submission new_wallet/S1 observed transaction lacks tx_id")?;
            let amount = constructed_amounts
                .get(tx_id)
                .and_then(Value::as_u64)
                .with_context(|| {
                    format!(
                        "submission new_wallet/S1 observed transaction {tx_id} lacks constructed amount evidence"
                    )
                })?;
            if transaction["amount_microtari"].as_u64() != Some(amount) {
                bail!(
                    "submission new_wallet/S1 observed transaction amount for {tx_id} does not match its constructed amount"
                );
            }
        }
    }
    Ok(())
}

fn validate_transaction_observations(
    mode: &str,
    scenario: &str,
    metrics: &Value,
) -> anyhow::Result<()> {
    let observations = metrics["transaction_observations"]
        .as_array()
        .with_context(|| {
            format!("submission {mode}/{scenario} missing transaction observations")
        })?;
    let preflight_failures = validate_s1_preflight_failures(mode, scenario, metrics)?;
    if observations.is_empty() {
        if preflight_failures == 0 {
            bail!("submission {mode}/{scenario} has no transaction observations");
        }
        validate_s1_constructed_output_shapes(mode, scenario, metrics, observations)?;
        validate_s1_constructed_amounts(mode, scenario, metrics, observations)?;
        return Ok(());
    }
    validate_s1_constructed_output_shapes(mode, scenario, metrics, observations)?;
    validate_s1_constructed_amounts(mode, scenario, metrics, observations)?;
    if mode == "payment_processor" {
        let confirmed = observations
            .iter()
            .filter(|observation| observation["terminal_outcome"] == "confirmed")
            .collect::<Vec<_>>();
        if !confirmed.is_empty() {
            let identities = metrics["batch_chain_identities"]
                .as_array()
                .with_context(|| {
                    format!("submission {mode}/{scenario} lacks batch-to-chain identities")
                })?;
            for observation in confirmed {
                let tx_id = observation["transaction_id"].as_str().unwrap_or_default();
                let batch_id = observation["batch_id"].as_str().unwrap_or_default();
                if tx_id.is_empty()
                    || batch_id.is_empty()
                    || !identities.iter().any(|identity| {
                        identity["batch_id"] == batch_id && identity["transaction_id"] == tx_id
                    })
                {
                    bail!(
                        "submission {mode}/{scenario} confirmed transaction lacks UUID-to-chain mapping"
                    );
                }
            }
        }
    }
    for observation in observations {
        let outcome = observation["terminal_outcome"].as_str().unwrap_or_default();
        if !matches!(
            outcome,
            "confirmed" | "rejected" | "stalled" | "timed_out" | "unavailable"
        ) {
            bail!("submission {mode}/{scenario} has an unknown terminal outcome");
        }
        let api_accepted = observation["api_accepted"].as_bool();
        let submission_started = observation["submission_started"].as_bool();
        let has_identity = if mode == "payment_processor" {
            observation["batch_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        } else {
            observation["transaction_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        };
        let has_attempt_identity = observation["attempt_index"].as_u64().is_some()
            || observation["batch_index"].as_u64().is_some();
        let has_recipients = observation["recipients"]
            .as_array()
            .is_some_and(|recipients| {
                !recipients.is_empty()
                    && recipients
                        .iter()
                        .all(|recipient| recipient.as_str().is_some_and(|value| !value.is_empty()))
            });
        let has_error = observation["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty());
        let has_reason = |field: &str| {
            observation[field]
                .as_str()
                .is_some_and(|reason| !reason.is_empty())
        };
        let has_explicit_timing_availability = (observation["construction_ms"].as_u64().is_some()
            || has_reason("construction_timing_reason"))
            && (observation["confirmation_ms"].as_u64().is_some()
                || has_reason("confirmation_timing_reason"))
            && (observation["mempool_available"].as_bool().is_some()
                || has_reason("mempool_reason"));
        let honest_arm_deadline = scenario == "S4"
            && outcome == "timed_out"
            && api_accepted.is_none()
            && observation["failure_class"] == "arm_deadline"
            && observation["deadline_expired"] == true
            && observation["submit_offset_ms"].as_u64().is_some()
            && has_attempt_identity
            && has_recipients
            && has_error
            && has_explicit_timing_availability;
        let honest_started_ambiguous_submission = matches!(outcome, "stalled" | "timed_out")
            && api_accepted.is_none()
            && submission_started == Some(true)
            && has_identity
            && observation["submission_ms"].as_u64().is_some()
            && observation["submission_timing_origin"]
                .as_str()
                .is_some_and(|origin| origin.ends_with("_ambiguous"))
            && matches!(
                observation["failure_class"].as_str(),
                Some("request_timeout" | "transport" | "response_decode" | "response_shape")
            )
            && has_error;
        let honest_new_wallet_pre_submission_failure = mode == "new_wallet"
            && outcome == "stalled"
            && api_accepted.is_none()
            && !has_identity
            && observation["submission_started"] == false
            && observation["deadline_expired"] == false
            && matches!(
                observation["failure_class"].as_str(),
                Some("wallet" | "database" | "process")
            )
            && observation["submit_offset_ms"].as_u64().is_some()
            && has_attempt_identity
            && has_recipients
            && has_error
            && has_explicit_timing_availability;
        let honest_payment_processor_pre_acceptance_failure = mode == "payment_processor"
            && outcome == "stalled"
            && api_accepted.is_none()
            && !has_identity
            && observation["deadline_expired"] == false
            && observation["submit_offset_ms"].as_u64().is_some()
            && has_attempt_identity
            && has_recipients
            && has_error
            && has_explicit_timing_availability
            && matches!(
                (
                    submission_started,
                    observation["failure_class"].as_str(),
                    observation["submission_timing_origin"].as_str(),
                    observation["submission_ms"].as_u64(),
                ),
                (
                    Some(false),
                    Some("wallet"),
                    Some("payment_processor_pre_api"),
                    None
                ) | (
                    Some(true),
                    Some("transport"),
                    Some("payment_processor_api_request_ambiguous"),
                    Some(_),
                ) | (
                    Some(true),
                    Some("response_decode" | "response_shape"),
                    Some("payment_processor_api_response_ambiguous"),
                    Some(_),
                )
            );
        let honest_old_wallet_ambiguous_submission = mode == "old_wallet"
            && outcome == "stalled"
            && api_accepted.is_none()
            && !has_identity
            && submission_started == Some(true)
            && observation["deadline_expired"] == false
            && observation["failure_class"] == "transport"
            && observation["submit_offset_ms"].as_u64().is_some()
            && observation["submission_ms"].as_u64().is_some()
            && observation["submission_timing_origin"]
                .as_str()
                .is_some_and(|origin| origin.ends_with("_ambiguous"))
            && has_attempt_identity
            && has_recipients
            && has_error
            && has_explicit_timing_availability;
        let reconciled_mode1_not_found = mode == "old_wallet"
            && outcome == "confirmed"
            && observation["transaction_id"]
                .as_str()
                .zip(observation["api_error"].as_str())
                .is_some_and(|(tx_id, error)| {
                    error.contains(&format!(
                        "status: NotFound, message: \"Transaction {tx_id} not found within timeout\""
                    ))
                });
        if api_accepted == Some(false) && outcome != "rejected" && !reconciled_mode1_not_found {
            bail!("submission {mode}/{scenario} immediate API failure must be rejected");
        }
        if matches!(outcome, "stalled" | "timed_out")
            && !((api_accepted == Some(true) && has_identity)
                || honest_arm_deadline
                || honest_started_ambiguous_submission
                || honest_new_wallet_pre_submission_failure
                || honest_payment_processor_pre_acceptance_failure
                || honest_old_wallet_ambiguous_submission)
        {
            bail!(
                "submission {mode}/{scenario} stalled observation lacks accepted operation identity"
            );
        }
        if outcome == "confirmed"
            && (api_accepted != Some(true) || !has_identity)
            && !reconciled_mode1_not_found
        {
            bail!(
                "submission {mode}/{scenario} confirmed observation lacks accepted operation identity"
            );
        }
        let construction_ms = observation["construction_ms"].as_u64();
        let construction_offset = observation["construction_complete_offset_ms"].as_u64();
        let construction_origin = observation["construction_timing_origin"]
            .as_str()
            .unwrap_or_default();
        match mode {
            "new_wallet" => match (construction_ms, construction_offset) {
                (Some(_), Some(_)) if construction_origin.starts_with("library_build_and_sign") => {
                }
                (None, None) if has_reason("construction_timing_reason") => {}
                _ => bail!(
                    "submission {mode}/{scenario} transaction is missing real construction timing or an explicit unavailable reason"
                ),
            },
            "payment_processor" => match (construction_ms, construction_offset) {
                (Some(_), Some(_))
                    if construction_origin
                        == "payment_processor_db_unsigned_tx_first_observation" => {}
                (None, None) if has_reason("construction_timing_reason") => {}
                _ => bail!(
                    "submission {mode}/{scenario} has invalid persisted-worker construction timing"
                ),
            },
            _ => {
                if construction_ms.is_some()
                    || construction_offset.is_some()
                    || !has_reason("construction_timing_reason")
                {
                    bail!(
                        "submission {mode}/{scenario} must disclose unavailable internal construction time"
                    );
                }
            }
        }
        if scenario == "S4" && observation["submit_offset_ms"].as_u64().is_none() {
            bail!("submission {mode}/{scenario} attempt is missing a common-clock submit offset");
        }
        if scenario == "S4" && !has_recipients {
            bail!("submission {mode}/{scenario} attempt is missing recipient identity");
        }
        if mode == "new_wallet" && outcome == "confirmed" {
            if observation["broadcast_start_offset_ms"].as_u64().is_none()
                || observation["confirmation_ms"].as_u64().is_none()
                || observation["confirmation_timing_origin"]
                    != "wallet_broadcast_start_to_independent_c_min"
            {
                bail!(
                    "submission {mode}/{scenario} confirmed transaction has invalid timing origins"
                );
            }
        } else if mode != "new_wallet" && outcome == "confirmed" {
            let origin = observation["confirmation_timing_origin"]
                .as_str()
                .unwrap_or_default();
            if observation["confirmation_ms"].as_u64().is_none()
                || !matches!(
                    origin,
                    "grpc_dispatch_to_independent_c_min" | "pp_api_acceptance_to_independent_c_min"
                )
            {
                bail!(
                    "submission {mode}/{scenario} confirmed transaction lacks honest dispatch/API-to-C-min timing"
                );
            }
        }
        if outcome == "confirmed"
            && (observation["fee_microtari"].as_u64().is_none()
                || observation["mined_height"].as_u64().is_none()
                || observation["tip_end_height"].as_u64().is_none()
                || observation["output_commitments"]
                    .as_array()
                    .is_none_or(Vec::is_empty))
        {
            bail!(
                "submission {mode}/{scenario} confirmed transaction is missing timing/fee/tip evidence"
            );
        }
        if outcome != "confirmed" && observation["error"].as_str().unwrap_or_default().is_empty() {
            bail!("submission {mode}/{scenario} failed transaction is missing an error reason");
        }
        if observation["fee_microtari"].is_null()
            && observation["fee_unavailable_reason"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        {
            bail!("submission {mode}/{scenario} transaction with unknown fee lacks a reason");
        }
        let fee_disposition = observation["fee_disposition"].as_str().unwrap_or_default();
        if observation
            .get("fee_disposition")
            .and_then(Value::as_str)
            .is_none()
        {
            bail!("submission {mode}/{scenario} observation must declare fee disposition");
        }
        if outcome == "confirmed" && fee_disposition != "confirmed_paid" {
            bail!("submission {mode}/{scenario} confirmed transaction fee is not marked paid");
        }
        if outcome == "rejected" && fee_disposition != "rejected" {
            bail!("submission {mode}/{scenario} rejected transaction has invalid fee disposition");
        }
        if matches!(outcome, "stalled" | "timed_out")
            && !matches!(fee_disposition, "proposed_unresolved" | "unavailable")
        {
            bail!(
                "submission {mode}/{scenario} unresolved transaction has invalid fee disposition"
            );
        }
        if outcome == "unavailable" && fee_disposition != "unavailable" {
            bail!(
                "submission {mode}/{scenario} unavailable transaction has invalid fee disposition"
            );
        }
        if fee_disposition == "proposed_unresolved"
            && (observation["fee_microtari"].as_u64().is_none()
                || !matches!(outcome, "stalled" | "timed_out"))
        {
            bail!("submission {mode}/{scenario} proposed unresolved fee is inconsistent");
        }
        if matches!(outcome, "stalled" | "timed_out")
            && fee_disposition == "unavailable"
            && observation["fee_microtari"].as_u64().is_some()
        {
            bail!(
                "submission {mode}/{scenario} unresolved transaction exposes a proposed fee but marks it unavailable"
            );
        }
    }
    Ok(())
}

fn validate_s1_exact_outputs(document: &Value) -> anyhow::Result<()> {
    for (mode, scenarios) in mode_scenarios(document)? {
        let s1 = &scenarios["S1"];
        if s1["outcome_status"] != "success" {
            continue;
        }
        let exact = s1["repetitions"]
            .as_array()
            .and_then(|runs| runs.last())
            .and_then(|run| run.get("metrics"))
            .and_then(|metrics| {
                metrics
                    .get("unspent_after")
                    .or_else(|| metrics.get("extra")?.get("unspent_after"))
            })
            .and_then(Value::as_u64);
        if exact != Some(512) {
            bail!("successful {mode}/S1 must prove unspent_after=512");
        }
    }
    Ok(())
}

fn validate_s5_comparisons(document: &Value) -> anyhow::Result<()> {
    let s5 = &document["computed_deltas"]["s5_throughput"];
    let arms = &s5["arms"];
    let comparisons = &s5["comparisons"];
    let cases = [
        (
            "new_wallet_individual_over_payment_processor_batch",
            ("new_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
        (
            "old_wallet_individual_over_payment_processor_batch",
            ("old_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
        (
            "new_wallet_individual_fee_per_recipient_over_payment_processor_batch",
            ("new_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
        (
            "old_wallet_individual_fee_per_recipient_over_payment_processor_batch",
            ("old_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
    ];
    for (name, left, right) in cases {
        if (!arm_complete(arms, left.0, left.1) || !arm_complete(arms, right.0, right.1))
            && !comparisons[name].is_null()
        {
            bail!("S5 comparison {name} must be null when a source arm is incomplete");
        }
    }
    let fee_cases = [
        (
            "new_wallet_individual_fee_per_recipient_over_payment_processor_batch",
            ("new_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
        (
            "old_wallet_individual_fee_per_recipient_over_payment_processor_batch",
            ("old_wallet", "individual"),
            ("payment_processor", "batch"),
        ),
    ];
    for (name, left, right) in fee_cases {
        let available = arm_complete(arms, left.0, left.1)
            && arm_complete(arms, right.0, right.1)
            && arms[left.0][left.1]["fee_per_recipient_microtari"]
                .as_f64()
                .is_some()
            && arms[right.0][right.1]["fee_per_recipient_microtari"]
                .as_f64()
                .is_some();
        if available == comparisons[name].is_null() {
            bail!("S5 fee comparison {name} does not match source fee availability");
        }
    }
    for mode in ["old_wallet", "new_wallet", "payment_processor"] {
        if let Some(mode_arms) = arms[mode].as_object() {
            if mode_arms.is_empty() {
                continue;
            }
            let self_send = mode_arms
                .get("self_send")
                .with_context(|| format!("submission {mode}/S5 missing self_send scope arm"))?;
            if self_send["complete"] != false
                || self_send["unavailable_reason"]
                    .as_str()
                    .is_none_or(str::is_empty)
            {
                bail!("submission {mode}/S5 self_send must be disclosed as unavailable");
            }
        }
    }
    Ok(())
}

fn arm_complete(arms: &Value, mode: &str, arm: &str) -> bool {
    let arm = &arms[mode][arm];
    arm["recipient_count"]
        .as_u64()
        .is_some_and(|count| count > 0)
        && arm["complete"] == true
}

fn mode_scenarios(
    document: &Value,
) -> anyhow::Result<Vec<(&str, &serde_json::Map<String, Value>)>> {
    document["modes"]
        .as_object()
        .context("modes must be an object")?
        .iter()
        .map(|(mode, value)| {
            value["scenarios"]
                .as_object()
                .map(|scenarios| (mode.as_str(), scenarios))
                .with_context(|| format!("{mode}.scenarios must be an object"))
        })
        .collect()
}

fn summary_outcome_counts(cell: &Value) -> (usize, usize, usize, usize, usize) {
    let observations = cell["repetitions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|run| run["metrics"]["transaction_observations"].as_array())
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let counts = OutcomeCounts::from_json_observations(&observations);
    (
        counts.accepted as usize,
        counts.confirmed as usize,
        counts.rejected as usize,
        counts.stalled as usize,
        counts.timed_out as usize,
    )
}

pub fn write_summary(profile_path: &Path, output_path: &Path) -> anyhow::Result<()> {
    let bytes =
        fs::read(profile_path).with_context(|| format!("reading {}", profile_path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", profile_path.display()))?;
    let markdown = render_summary(&document)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, markdown)
        .with_context(|| format!("writing {}", output_path.display()))?;
    Ok(())
}

pub fn write_legacy_v5_summary(profile_path: &Path, output_path: &Path) -> anyhow::Result<()> {
    let bytes =
        fs::read(profile_path).with_context(|| format!("reading {}", profile_path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", profile_path.display()))?;
    validate_legacy_v5_document(&document)?;
    let mut output = String::new();
    output.push_str("# Tari Wallet Benchmark Result\n\n");
    output.push_str(&format!(
        "- Run ID: `{}`\n",
        markdown_text(&document["run_id"])
    ));
    output.push_str(&format!(
        "- Profile: `{}`\n",
        markdown_text(&document["profile_kind"])
    ));
    output.push_str(&format!("- Complete: `{}`\n", document["run_complete"]));
    output.push_str(&format!(
        "- Network: `{}`\n",
        markdown_text(&document["network"])
    ));
    output.push_str(&format!(
        "- Harness commit: `{}`\n",
        markdown_text(&document["harness_git_commit"])
    ));
    output.push_str(&format!(
        "- Selected scan node: `{}` (`{}`; `{}`)\n",
        markdown_text(&document["base_node"]["endpoint"]),
        markdown_text(&document["base_node"]["configured_revision"]),
        markdown_text(&document["environment"]["base_node_network_path"])
    ));
    output.push_str(&format!(
        "- Independent authority: `{}` (`{}`)\n\n",
        markdown_text(&document["base_node"]["authority_endpoint"]),
        markdown_text(&document["environment"]["authority_network_path"])
    ));
    output
        .push_str("| Mode | Scenario | Execution | Outcome | Median ms | Successes | Failures |\n");
    output.push_str("|---|---:|---|---|---:|---:|---:|\n");
    for mode in ModeName::ALL {
        for scenario in ScenarioName::ALL {
            let cell = &document["modes"][mode.as_str()]["scenarios"][scenario.as_str()];
            let (successes, failures) = cell["repetitions"]
                .as_array()
                .and_then(|runs| runs.last())
                .map(|run| (&run["success_count"], &run["failure_count"]))
                .unwrap_or((&Value::Null, &Value::Null));
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                mode.as_str(),
                scenario.as_str(),
                markdown_text(&cell["execution_status"]),
                markdown_text(&cell["outcome_status"]),
                display_value(&cell["median_wall_ms"]),
                display_value(successes),
                display_value(failures)
            ));
        }
    }
    output.push_str(&format!(
        "\nConfirmed top-level transactions: **{}**\n",
        document["chain_verification"]["verified_transactions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default()
    ));
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, output).with_context(|| format!("writing {}", output_path.display()))?;
    Ok(())
}

pub fn write_legacy_v6_summary(profile_path: &Path, output_path: &Path) -> anyhow::Result<()> {
    let bytes =
        fs::read(profile_path).with_context(|| format!("reading {}", profile_path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as JSON", profile_path.display()))?;
    validate_legacy_v6_document(&document)?;
    let markdown = render_summary_document(&document);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, markdown)
        .with_context(|| format!("writing {}", output_path.display()))?;
    Ok(())
}

pub fn render_summary(document: &Value) -> anyhow::Result<String> {
    validate_document(document, false)?;
    Ok(render_summary_document(document))
}

fn render_summary_document(document: &Value) -> String {
    let mut output = String::new();
    output.push_str("# Tari Wallet Benchmark Result\n\n");
    output.push_str(&format!(
        "- Run ID: `{}`\n",
        markdown_text(&document["run_id"])
    ));
    output.push_str(&format!(
        "- Profile: `{}`\n",
        markdown_text(&document["profile_kind"])
    ));
    output.push_str(&format!("- Complete: `{}`\n", document["run_complete"]));
    output.push_str(&format!(
        "- Network: `{}`\n",
        markdown_text(&document["network"])
    ));
    output.push_str(&format!(
        "- Measurement commit: `{}`\n",
        markdown_text(&document["provenance"]["measurement_commit"])
    ));
    output.push_str(&format!(
        "- Export commit: `{}`\n",
        markdown_text(&document["provenance"]["export_commit"])
    ));
    if !document["provenance"]["measurement_worktree_clean"].is_null()
        || !document["provenance"]["export_worktree_clean"].is_null()
    {
        output.push_str(&format!(
            "- Harness worktree clean: measurement=`{}`, export=`{}`\n",
            document["provenance"]["measurement_worktree_clean"],
            document["provenance"]["export_worktree_clean"]
        ));
    }
    output.push_str(&format!(
        "- Selected scan node: `{}` (`{}`; `{}`)\n",
        markdown_text(&document["base_node"]["endpoint"]),
        markdown_text(&document["base_node"]["configured_revision"]),
        markdown_text(&document["environment"]["base_node_network_path"])
    ));
    output.push_str(&format!(
        "- Independent authority: `{}` (`{}`)\n\n",
        markdown_text(&document["base_node"]["authority_endpoint"]),
        markdown_text(&document["environment"]["authority_network_path"])
    ));
    output.push_str("| Mode | Scenario | Execution | Outcome | Median ms (ok) | Median ms (all) | API accepted | Chain confirmed | Rejected | Stalled | Timed out | Successes | Failures |\n");
    output.push_str("|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for mode in ModeName::ALL {
        for scenario in ScenarioName::ALL {
            let cell = &document["modes"][mode.as_str()]["scenarios"][scenario.as_str()];
            let (successes, failures) = cell["repetitions"]
                .as_array()
                .and_then(|runs| runs.last())
                .map(|run| (&run["success_count"], &run["failure_count"]))
                .unwrap_or((&Value::Null, &Value::Null));
            let (api_accepted, confirmed, rejected, stalled, timed_out) =
                summary_outcome_counts(cell);
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                mode.as_str(),
                scenario.as_str(),
                markdown_text(&cell["execution_status"]),
                markdown_text(&cell["outcome_status"]),
                display_value(&cell["median_wall_ms"]),
                display_value(&cell["all_runs_median_wall_ms"]),
                api_accepted,
                confirmed,
                rejected,
                stalled,
                timed_out,
                display_value(successes),
                display_value(failures)
            ));
        }
    }
    output.push_str(&format!(
        "\nConfirmed top-level transactions: **{}**\n",
        document["chain_verification"]["verified_transactions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default()
    ));
    output
}

fn markdown_text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .replace('|', "\\|")
        .replace('`', "'")
}

fn display_value(value: &Value) -> String {
    if value.is_null() {
        "—".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        env_capture,
        result_profile::{VerifiedTransaction, empty_mode_profile},
    };

    fn profile_document() -> Value {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, env_capture::capture());
        for mode in ModeName::ALL {
            profile.modes.insert(
                mode.as_str().to_string(),
                empty_mode_profile(mode, Some(format!("{mode:?}-address"))),
            );
        }
        profile.refresh_computed_deltas();
        let mut document = serde_json::to_value(profile).unwrap();
        let manifest =
            serde_json::to_value(crate::build_manifest::portable_manifest_fixture()).unwrap();
        document["provenance"]["measurement_build_manifest"] = manifest.clone();
        document["provenance"]["export_build_manifest"] = manifest.clone();
        document["config"]["build_manifest"] = manifest;
        document["config"]["seed_fingerprints"] = json!({
            "old_wallet": "old",
            "new_wallet": "new",
            "payment_processor": "payment",
            "recipient_sink": "sink"
        });
        document
    }

    #[test]
    fn current_checkpoint_round_trips() {
        let document = profile_document();
        let profile = validate_document(&document, false).unwrap();
        assert_eq!(profile.schema_version, RESULT_SCHEMA_VERSION);
    }

    #[test]
    fn legacy_v5_validation_is_explicit_and_does_not_accept_v6() {
        let mut legacy = profile_document();
        legacy["schema_version"] = json!(5);
        legacy["profile_kind"] = json!("final");
        legacy["run_complete"] = json!(true);
        legacy["harness_git_commit"] = json!("historical");
        legacy.as_object_mut().unwrap().remove("provenance");
        validate_legacy_v5_document(&legacy).unwrap();

        legacy["schema_version"] = json!(LEGACY_RESULT_SCHEMA_VERSION);
        assert!(validate_legacy_v5_document(&legacy).is_err());
    }

    #[test]
    fn schema_v6_profile_requires_explicit_legacy_path() {
        let mut document = profile_document();
        document["schema_version"] = json!(LEGACY_RESULT_SCHEMA_VERSION);
        document["base_node"]
            .as_object_mut()
            .unwrap()
            .remove("pruned_height");
        document["config"]
            .as_object_mut()
            .unwrap()
            .remove("seed_fingerprints");
        document["provenance"]
            .as_object_mut()
            .unwrap()
            .remove("measurement_worktree_clean");
        document["provenance"]
            .as_object_mut()
            .unwrap()
            .remove("export_worktree_clean");
        document["provenance"]
            .as_object_mut()
            .unwrap()
            .remove("measurement_build_worktree_clean");
        document["provenance"]
            .as_object_mut()
            .unwrap()
            .remove("export_build_worktree_clean");
        document["config"]
            .as_object_mut()
            .unwrap()
            .remove("export_harness_executable_sha256");
        assert!(document["base_node"].get("pruned_height").is_none());
        assert!(validate_document(&document, false).is_err());
        assert!(validate_document(&document, true).is_err());
        validate_legacy_v6_document(&document).unwrap();
    }

    #[test]
    fn current_schema_requires_exact_typed_build_manifest_sets() {
        let canonical = profile_document();
        let mut missing_source = canonical.clone();
        missing_source["provenance"]["measurement_build_manifest"]["sources"]
            .as_object_mut()
            .unwrap()
            .remove("minotari_node");
        let mut extra_artifact = canonical.clone();
        extra_artifact["provenance"]["measurement_build_manifest"]["artifacts"]["miner"] =
            extra_artifact["provenance"]["measurement_build_manifest"]["artifacts"]["minotari"]
                .clone();
        let mut wrong_patch_count = canonical;
        wrong_patch_count["provenance"]["measurement_build_manifest"]["sources"]["tari_console_wallet"]
            ["patches"] = json!([wrong_patch_count["provenance"]
            ["measurement_build_manifest"]["sources"]["minotari_cli"]["patches"][0]
            .clone()]);

        for document in [missing_source, extra_artifact, wrong_patch_count] {
            assert!(validate_document(&document, false).is_err());
        }
    }

    #[test]
    fn current_schema_requires_recipient_sink_fingerprint() {
        let mut document = profile_document();
        document["config"]["seed_fingerprints"]
            .as_object_mut()
            .unwrap()
            .remove("recipient_sink");
        assert!(validate_document(&document, false).is_err());
    }

    #[test]
    fn submission_rejects_tuned_payment_processor_worker_cadence() {
        let mut document = profile_document();
        let profile = validate_document(&document, false).unwrap();
        document["config"]["mode3_batch_creator_sleep_secs"] = json!(10);

        let error = validate_reference_configuration(&profile, &document)
            .unwrap_err()
            .to_string();
        assert!(error.contains("mode3_batch_creator_sleep_secs"));
    }

    #[test]
    fn completed_repetitions_require_fee_and_balance_evidence() {
        let document = serde_json::json!({
            "modes": {
                "old_wallet": {
                    "scenarios": {
                        "S0": {
                            "repetitions": [{
                                "execution_status": "completed",
                                "wall_ms": 1,
                                "fee_microtari": 0,
                                "metrics": {
                                    "balance_reconciliation_unavailable_reason": "pre-run timing is unavailable"
                                }
                            }]
                        }
                    }
                }
            }
        });
        validate_cross_cutting_scenario_metrics(&document).unwrap();

        let mut missing_fee = document.clone();
        missing_fee["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0]["fee_microtari"] =
            Value::Null;
        assert!(
            validate_cross_cutting_scenario_metrics(&missing_fee)
                .unwrap_err()
                .to_string()
                .contains("explicit fees")
        );

        let mut missing_balance = document.clone();
        missing_balance["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0]["metrics"] =
            serde_json::json!({});
        assert!(
            validate_cross_cutting_scenario_metrics(&missing_balance)
                .unwrap_err()
                .to_string()
                .contains("final balance reconciliation")
        );

        let mut s0_reconciliation = document;
        s0_reconciliation["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0]["metrics"] = serde_json::json!({
            "balance_reconciliation": {
                "assumption": "S0 expected balance equals A_fund; funding fee is outside measurement",
                "expected_balance_microtari": 10_000,
                "observed_balance_microtari": 10_000,
                "delta_microtari": 0,
                "flagged": false
            }
        });
        validate_cross_cutting_scenario_metrics(&s0_reconciliation).unwrap();
    }

    #[test]
    fn balance_reconciliation_rejects_arithmetic_mutation() {
        let metrics = json!({
            "balance_before_microtari": 1000,
            "balance_after_microtari": 300,
            "outgoing_microtari": 600,
            "balance_reconciliation": {
                "expected_balance_microtari": 300,
                "observed_balance_microtari": 300,
                "delta_microtari": 0,
                "flagged": false,
                "balance_domain": "available"
            }
        });
        validate_balance_reconciliation(metrics.as_object().unwrap(), "test", Some(100), 600)
            .unwrap();
        let mut mutated = metrics.clone();
        mutated["balance_reconciliation"]["delta_microtari"] = json!(1);
        assert!(
            validate_balance_reconciliation(mutated.as_object().unwrap(), "test", Some(100), 600,)
                .is_err()
        );
        let mut underflow = metrics.clone();
        underflow["balance_before_microtari"] = json!(50);
        assert!(validate_balance_reconciliation(
            underflow.as_object().unwrap(),
            "test",
            Some(100),
            600,
        )
        .is_err());
        let mut missing_domain = metrics.clone();
        missing_domain["balance_reconciliation"]["balance_domain"] = Value::Null;
        assert!(
            validate_balance_reconciliation(
                missing_domain.as_object().unwrap(),
                "test",
                Some(100),
                600,
            )
            .is_err()
        );
        assert!(
            validate_balance_reconciliation(metrics.as_object().unwrap(), "test", Some(100), 599,)
                .unwrap_err()
                .to_string()
                .contains("confirmed observation amounts")
        );
    }

    #[test]
    fn self_directed_s1_has_zero_external_outgoing() {
        assert_eq!(
            reconciliation_outgoing_microtari("old_wallet", "S1", 10_000),
            0
        );
        assert_eq!(
            reconciliation_outgoing_microtari("new_wallet", "S1", 10_000),
            0
        );
        assert_eq!(
            reconciliation_outgoing_microtari("payment_processor", "S1", 10_000),
            0
        );
        assert_eq!(
            reconciliation_outgoing_microtari("old_wallet", "S4", 10_000),
            10_000
        );
    }

    #[test]
    fn mode1_not_found_response_can_reconcile_to_confirmed_chain_evidence() {
        let metrics = json!({
            "transaction_observations": [{
                "transaction_id": "42",
                "submit_offset_ms": 0,
                "construction_complete_offset_ms": null,
                "recipients": ["recipient"],
                "api_accepted": false,
                "api_error": "status: NotFound, message: \"Transaction 42 not found within timeout\"",
                "terminal_outcome": "confirmed",
                "construction_ms": null,
                "construction_timing_reason": "console-wallet gRPC does not expose internal construction completion",
                "confirmation_ms": 100,
                "confirmation_timing_origin": "grpc_dispatch_to_independent_c_min",
                "fee_microtari": 660,
                "fee_disposition": "confirmed_paid",
                "mined_height": 10,
                "tip_end_height": 13,
                "output_commitments": ["commitment"],
                "error": null
            }]
        });
        validate_transaction_observations("old_wallet", "S4", &metrics).unwrap();

        let mut unrelated = metrics;
        unrelated["transaction_observations"][0]["api_error"] =
            json!("status: NotFound, message: \"Transaction 43 not found within timeout\"");
        assert!(validate_transaction_observations("old_wallet", "S4", &unrelated).is_err());
    }

    #[test]
    fn honest_s4_arm_deadline_partial_is_valid_but_tampering_is_rejected() {
        let observation = json!({
            "transaction_id": null,
            "attempt_index": null,
            "batch_index": 7,
            "batch_id": null,
            "submit_offset_ms": 21,
            "construction_complete_offset_ms": null,
            "construction_ms": null,
            "construction_timing_reason": "construction completion was unavailable before the arm deadline",
            "submission_ms": null,
            "mempool_available": null,
            "mempool_reason": "the request did not return an acceptance result before the arm deadline",
            "confirmation_ms": null,
            "confirmation_timing_reason": "C_min was not observed before the arm deadline",
            "fee_microtari": null,
            "fee_unavailable_reason": "no independently verified fee was available",
            "fee_disposition": "unavailable",
            "recipients": ["recipient"],
            "api_accepted": null,
            "failure_class": "arm_deadline",
            "deadline_expired": true,
            "terminal_outcome": "timed_out",
            "error": "absolute arm deadline expired"
        });
        for mode in ["old_wallet", "new_wallet", "payment_processor"] {
            let metrics = json!({"transaction_observations": [observation.clone()]});
            validate_transaction_observations(mode, "S4", &metrics).unwrap();
        }

        for (field, tampered) in [
            ("deadline_expired", json!(false)),
            ("failure_class", json!("transport")),
            ("error", Value::Null),
            ("submit_offset_ms", Value::Null),
            ("batch_index", Value::Null),
            ("recipients", json!([])),
            ("construction_timing_reason", Value::Null),
        ] {
            let mut metrics = json!({"transaction_observations": [observation.clone()]});
            metrics["transaction_observations"][0][field] = tampered;
            assert!(
                validate_transaction_observations("old_wallet", "S4", &metrics).is_err(),
                "tampered {field} unexpectedly passed"
            );
        }
    }

    #[test]
    fn identity_bearing_started_submission_may_remain_ambiguous() {
        let observation = json!({
            "transaction_id": "tx",
            "attempt_index": 1,
            "batch_index": null,
            "submit_offset_ms": 5,
            "construction_complete_offset_ms": 10,
            "construction_ms": 5,
            "construction_timing_origin": "library_build_and_sign_attempt",
            "submission_ms": 30,
            "submission_timing_origin": "base_node_submit_transaction_ambiguous",
            "submission_started": true,
            "mempool_available": null,
            "mempool_reason": "submission result was ambiguous",
            "confirmation_ms": null,
            "confirmation_timing_reason": "C_min was not observed",
            "fee_microtari": 700,
            "fee_disposition": "proposed_unresolved",
            "recipients": ["recipient"],
            "api_accepted": null,
            "failure_class": "request_timeout",
            "deadline_expired": false,
            "terminal_outcome": "timed_out",
            "error": "request timed out after submission started"
        });
        let metrics = json!({"transaction_observations": [observation.clone()]});
        validate_transaction_observations("new_wallet", "S4", &metrics).unwrap();

        let mut tampered = metrics;
        tampered["transaction_observations"][0]["submission_timing_origin"] =
            json!("base_node_submit_transaction");
        assert!(validate_transaction_observations("new_wallet", "S4", &tampered).is_err());
    }

    #[test]
    fn new_wallet_pre_submission_failure_is_not_faked_as_api_rejection() {
        let observation = json!({
            "transaction_id": null,
            "attempt_index": 1,
            "batch_index": null,
            "submit_offset_ms": 5,
            "construction_complete_offset_ms": null,
            "construction_ms": null,
            "construction_timing_reason": "wallet construction failed before producing a transaction",
            "submission_ms": null,
            "submission_started": false,
            "mempool_available": null,
            "mempool_reason": "base-node submission did not start",
            "confirmation_ms": null,
            "confirmation_timing_reason": "no transaction existed to confirm",
            "fee_microtari": null,
            "fee_unavailable_reason": "construction did not produce a fee",
            "fee_disposition": "unavailable",
            "recipients": ["recipient"],
            "api_accepted": null,
            "failure_class": "wallet",
            "deadline_expired": false,
            "terminal_outcome": "stalled",
            "error": "wallet input selection failed"
        });
        let metrics = json!({"transaction_observations": [observation.clone()]});
        validate_transaction_observations("new_wallet", "S4", &metrics).unwrap();

        for (field, tampered) in [
            ("api_accepted", json!(false)),
            ("submission_started", json!(true)),
            ("failure_class", json!("transport")),
            ("attempt_index", Value::Null),
            ("recipients", json!([])),
        ] {
            let mut metrics = json!({"transaction_observations": [observation.clone()]});
            metrics["transaction_observations"][0][field] = tampered;
            assert!(
                validate_transaction_observations("new_wallet", "S4", &metrics).is_err(),
                "tampered {field} unexpectedly passed"
            );
        }
    }

    #[test]
    fn pp_pre_acceptance_failures_require_truthful_started_state_and_origin() {
        let cases = [
            ("wallet", false, "payment_processor_pre_api", Value::Null),
            (
                "transport",
                true,
                "payment_processor_api_request_ambiguous",
                json!(7),
            ),
            (
                "response_decode",
                true,
                "payment_processor_api_response_ambiguous",
                json!(7),
            ),
            (
                "response_shape",
                true,
                "payment_processor_api_response_ambiguous",
                json!(7),
            ),
        ];
        for (failure_class, submission_started, submission_origin, submission_ms) in cases {
            let observation = json!({
                "transaction_id": null,
                "attempt_index": null,
                "batch_index": 1,
                "batch_id": null,
                "submit_offset_ms": 2,
                "construction_complete_offset_ms": null,
                "construction_ms": null,
                "construction_timing_reason": "no persisted unsigned transaction was observed",
                "submission_ms": submission_ms,
                "submission_timing_origin": submission_origin,
                "submission_started": submission_started,
                "mempool_available": null,
                "mempool_reason": "payment-processor acceptance was not established",
                "confirmation_ms": null,
                "confirmation_timing_reason": "no accepted batch existed to confirm",
                "fee_microtari": null,
                "fee_unavailable_reason": "no independently verified fee was available",
                "fee_disposition": "unavailable",
                "recipients": ["recipient"],
                "api_accepted": null,
                "failure_class": failure_class,
                "deadline_expired": false,
                "terminal_outcome": "stalled",
                "error": "payment-processor batch failed before acceptance"
            });
            let metrics = json!({"transaction_observations": [observation.clone()]});
            validate_transaction_observations("payment_processor", "S1", &metrics).unwrap();

            let mut wrong_started = metrics.clone();
            wrong_started["transaction_observations"][0]["submission_started"] =
                json!(!submission_started);
            assert!(
                validate_transaction_observations("payment_processor", "S1", &wrong_started)
                    .is_err(),
                "tampered submission_started unexpectedly passed for {failure_class}"
            );

            let mut wrong_origin = metrics;
            wrong_origin["transaction_observations"][0]["submission_timing_origin"] =
                json!("payment_processor_api_acceptance");
            assert!(
                validate_transaction_observations("payment_processor", "S1", &wrong_origin)
                    .is_err(),
                "tampered submission origin unexpectedly passed for {failure_class}"
            );
        }
    }

    #[test]
    fn s1_preflight_failure_is_reportable_without_a_fake_transaction() {
        let row = json!({
            "round_index": 1,
            "attempted_index": null,
            "stage": "read_spendable_inputs",
            "submission_started": false,
            "error": "wallet database could not enumerate spendable inputs"
        });
        for (mode, metrics) in [
            (
                "old_wallet",
                json!({
                    "preflight_failures": [row.clone()],
                    "constructed_output_shapes_by_tx_id": {},
                    "transaction_observations": []
                }),
            ),
            (
                "new_wallet",
                json!({
                    "preflight_failures": [row.clone()],
                    "constructed_output_shapes_by_tx_id": {},
                    "constructed_amounts_microtari_by_tx_id": {},
                    "transaction_observations": []
                }),
            ),
            (
                "payment_processor",
                json!({
                    "extra": {"preflight_failures": [row.clone()]},
                    "transaction_observations": []
                }),
            ),
        ] {
            validate_transaction_observations(mode, "S1", &metrics).unwrap();

            let mut fake_submission = metrics;
            if mode == "payment_processor" {
                fake_submission["extra"]["preflight_failures"][0]["submission_started"] =
                    json!(true);
            } else {
                fake_submission["preflight_failures"][0]["submission_started"] = json!(true);
            }
            assert!(validate_transaction_observations(mode, "S1", &fake_submission).is_err());
        }
    }

    #[test]
    fn old_wallet_ambiguous_grpc_failure_is_not_faked_as_rejection() {
        let observation = json!({
            "transaction_id": null,
            "attempt_index": null,
            "batch_index": 1,
            "submit_offset_ms": 5,
            "construction_complete_offset_ms": null,
            "construction_ms": null,
            "construction_timing_reason": "console wallet does not expose construction",
            "submission_ms": 20,
            "submission_timing_origin": "console_wallet_grpc_request_ambiguous",
            "submission_started": true,
            "mempool_available": null,
            "mempool_reason": "gRPC response was ambiguous",
            "confirmation_ms": null,
            "confirmation_timing_reason": "no transaction identity was returned",
            "fee_microtari": null,
            "fee_unavailable_reason": "no verified transaction identity",
            "fee_disposition": "unavailable",
            "recipients": ["recipient"],
            "api_accepted": null,
            "failure_class": "transport",
            "deadline_expired": false,
            "terminal_outcome": "stalled",
            "error": "connection lost after request dispatch"
        });
        let metrics = json!({"transaction_observations": [observation.clone()]});
        validate_transaction_observations("old_wallet", "S4", &metrics).unwrap();

        let mut fake_rejection = metrics;
        fake_rejection["transaction_observations"][0]["api_accepted"] = json!(false);
        assert!(validate_transaction_observations("old_wallet", "S4", &fake_rejection).is_err());
    }

    #[test]
    fn pp_persisted_unsigned_transaction_is_valid_genuine_construction_evidence() {
        let observation = json!({
            "transaction_id": null,
            "attempt_index": null,
            "batch_index": 1,
            "batch_id": "batch",
            "submit_offset_ms": 20,
            "construction_complete_offset_ms": 125,
            "construction_ms": 105,
            "construction_timing_origin": "payment_processor_db_unsigned_tx_first_observation",
            "construction_timing_reason": "first polling observation of persisted unsigned_tx_json",
            "submission_ms": 10,
            "submission_timing_origin": "payment_processor_api_acceptance",
            "submission_started": true,
            "mempool_available": null,
            "mempool_reason": "payment processor exposes no per-batch mempool timestamp",
            "confirmation_ms": null,
            "confirmation_timing_reason": "C_min was not observed",
            "fee_microtari": null,
            "fee_unavailable_reason": "no independently verified fee was available",
            "fee_disposition": "unavailable",
            "recipients": ["recipient"],
            "api_accepted": true,
            "deadline_expired": false,
            "terminal_outcome": "stalled",
            "error": "batch did not reach C_min"
        });
        let metrics = json!({"transaction_observations": [observation.clone()]});
        validate_transaction_observations("payment_processor", "S4", &metrics).unwrap();

        let mut tampered = metrics;
        tampered["transaction_observations"][0]["construction_timing_origin"] =
            json!("payment_processor_api_acceptance");
        assert!(validate_transaction_observations("payment_processor", "S4", &tampered).is_err());
    }

    #[test]
    fn malformed_profile_missing_key_fails_schema() {
        let mut document = profile_document();
        document.as_object_mut().unwrap().remove("run_id");
        assert!(validate_document(&document, false).is_err());
    }

    #[test]
    fn invalid_confirmation_row_fails_schema() {
        let mut document = profile_document();
        document["chain_verification"]["verified_transactions"] = json!([{
            "tx_id": "42", "status_value": 2, "mode": "old_wallet", "scenario": "S1",
            "mined_height": 100, "confirmations": 3, "min_confirmations": 3,
            "tip_height": 103, "confirmed": false
        }]);
        assert!(validate_document(&document, false).is_err());
    }

    #[test]
    fn confirmed_binding_covers_later_repetitions() {
        let observation = json!({"transaction_id": "42", "terminal_outcome": "confirmed"});
        let mut document = profile_document();
        document["modes"]["old_wallet"]["scenarios"]["S1"]["repetitions"] = json!([
            {"metrics": {"transaction_observations": [observation.clone()]}},
            {"metrics": {"transaction_observations": [observation]}}
        ]);
        let mut profile = ResultProfile::new(&Config::default(), env_capture::capture());
        profile
            .chain_verification
            .verified_transactions
            .push(VerifiedTransaction {
                tx_id: "42".to_string(),
                status_value: TX_MINED_CONFIRMED_STATUS,
                mode: "old_wallet".to_string(),
                scenario: "S1".to_string(),
                amount_microtari: None,
                fee_microtari: None,
                mined_height: Some(1),
                confirmations: Some(3),
                min_confirmations: Some(3),
                tip_height: Some(4),
                confirmed: true,
            });
        assert!(validate_confirmed_observation_bindings(&document, &profile).is_err());
    }

    #[test]
    fn pp_confirmed_binding_requires_the_same_batch_uuid() {
        let metrics = json!({
            "transaction_observations": [{
                "batch_id": "batch-a",
                "transaction_id": "tx-1",
                "terminal_outcome": "confirmed"
            }],
            "batch_chain_identities": [{
                "batch_id": "batch-b",
                "transaction_id": "tx-1"
            }]
        });
        assert!(validate_transaction_observations("payment_processor", "S1", &metrics).is_err());
    }

    #[test]
    fn s1_wallet_observation_roles_must_match_tx_id_keyed_construction_evidence() {
        let metrics = json!({
            "constructed_output_shapes_by_tx_id": {
                "tx-1": {
                    "payment_output_count": 1,
                    "change_output_count": 1,
                    "total_output_count": 2
                }
            },
            "transaction_observations": [{
                "terminal_outcome": "confirmed",
                "api_accepted": true,
                "transaction_id": "tx-1",
                "fee_microtari": 10,
                "fee_disposition": "confirmed_paid",
                "construction_timing_reason": "console wallet does not expose internal construction",
                "construction_ms": null,
                "construction_complete_offset_ms": null,
                "confirmation_ms": 1,
                "confirmation_timing_origin": "grpc_dispatch_to_independent_c_min",
                "mined_height": 1,
                "tip_end_height": 4,
                "input_count": 1,
                "payment_output_count": 1,
                "change_output_count": 1,
                "total_output_count": 2,
                "output_commitments": ["payment", "change"]
            }]
        });
        validate_transaction_observations("old_wallet", "S1", &metrics).unwrap();

        let mut mislabeled_observation = metrics.clone();
        mislabeled_observation["transaction_observations"][0]["payment_output_count"] = json!(2);
        assert!(
            validate_transaction_observations("old_wallet", "S1", &mislabeled_observation).is_err()
        );

        let mut mislabeled_construction = metrics.clone();
        mislabeled_construction["constructed_output_shapes_by_tx_id"]["tx-1"]["change_output_count"] =
            json!(0);
        assert!(
            validate_transaction_observations("old_wallet", "S1", &mislabeled_construction)
                .is_err()
        );
    }

    #[test]
    fn new_wallet_s1_amounts_must_match_tx_id_keyed_construction_evidence() {
        let metrics = json!({
            "constructed_amounts_microtari_by_tx_id": {"tx-1": 1234},
            "constructed_output_shapes_by_tx_id": {
                "tx-1": {
                    "payment_output_count": 1,
                    "change_output_count": 1,
                    "total_output_count": 2
                }
            },
            "observed_transactions": [{
                "tx_id": "tx-1",
                "amount_microtari": 1234
            }],
            "transaction_observations": [{
                "terminal_outcome": "confirmed",
                "api_accepted": true,
                "transaction_id": "tx-1",
                "amount_microtari": 1234,
                "fee_microtari": 10,
                "fee_disposition": "confirmed_paid",
                "construction_ms": 2,
                "construction_complete_offset_ms": 2,
                "construction_timing_origin": "library_build_and_sign",
                "confirmation_ms": 1,
                "confirmation_timing_origin": "wallet_broadcast_start_to_independent_c_min",
                "broadcast_start_offset_ms": 2,
                "mined_height": 1,
                "tip_end_height": 4,
                "input_count": 1,
                "payment_output_count": 1,
                "change_output_count": 1,
                "total_output_count": 2,
                "output_commitments": ["payment", "change"]
            }]
        });
        validate_transaction_observations("new_wallet", "S1", &metrics).unwrap();

        for path in ["observation", "verified", "map"] {
            let mut tampered = metrics.clone();
            match path {
                "observation" => {
                    tampered["transaction_observations"][0]["amount_microtari"] = json!(999)
                }
                "verified" => tampered["observed_transactions"][0]["amount_microtari"] = json!(999),
                "map" => tampered["constructed_amounts_microtari_by_tx_id"]["tx-1"] = json!(999),
                _ => unreachable!(),
            }
            assert!(
                validate_transaction_observations("new_wallet", "S1", &tampered).is_err(),
                "tampered {path} amount unexpectedly passed"
            );
        }
    }

    #[test]
    fn s1_round_observations_and_fees_are_bound_to_the_exact_round() {
        let mut observations = Vec::new();
        let mut summaries = Vec::new();
        let mut rounds = Vec::new();
        for (round_index, tx_count, outputs_per_tx, target_utxos_after) in REFERENCE_S1_ROUNDS {
            for _ in 0..tx_count {
                observations.push(json!({
                    "configured_batch": round_index,
                    "terminal_outcome": "confirmed",
                    "input_count": 1,
                    "payment_output_count": outputs_per_tx - 1,
                    "change_output_count": 1,
                    "total_output_count": outputs_per_tx,
                    "fee_microtari": 5
                }));
            }
            summaries.push(json!({
                "configured_batch": round_index,
                "attempted": tx_count,
                "wall_ms": 1
            }));
            rounds.push(json!({
                "round_index": round_index,
                "tx_count": tx_count,
                "outputs_per_tx": outputs_per_tx,
                "target_utxos_after": target_utxos_after,
                "total_fee_microtari": tx_count * 5
            }));
        }
        let metrics = json!({
            "transaction_observations": observations,
            "batch_summaries": summaries,
            "rounds": rounds
        });
        validate_s1_round_bindings("new_wallet", &metrics).unwrap();

        let mut moved_to_wrong_round = metrics.clone();
        moved_to_wrong_round["transaction_observations"][0]["configured_batch"] = json!(2);
        assert!(validate_s1_round_bindings("new_wallet", &moved_to_wrong_round).is_err());

        let mut false_round_fee = metrics;
        false_round_fee["rounds"][0]["total_fee_microtari"] = json!(0);
        assert!(validate_s1_round_bindings("new_wallet", &false_round_fee).is_err());
    }

    #[test]
    fn paired_scans_require_one_immutable_target_and_expected_state() {
        let metrics = json!({
            "H_tip_end": 100,
            "H_tip_target_hash": "hash",
            "scan_checkpoint": "post_s1",
            "expected_outputs": 512,
            "expected_available_microtari": 10,
            "expected_history_transactions": 127,
            "expected_history_tx_ids": ["tx"],
            "expected_output_commitments": {"tx": ["commitment"]}
        });
        let scenarios = json!({
            "S2": {
                "execution_status": "completed",
                "repetitions": [{"metrics": metrics.clone()}]
            },
            "S3": {
                "execution_status": "completed",
                "repetitions": [{"metrics": metrics}]
            }
        });
        validate_paired_scan_targets(
            "new_wallet",
            scenarios.as_object().unwrap(),
            "S2",
            "S3",
            "post_s1",
        )
        .unwrap();

        let mut changed_target = scenarios.clone();
        changed_target["S3"]["repetitions"][0]["metrics"]["H_tip_end"] = json!(101);
        assert!(
            validate_paired_scan_targets(
                "new_wallet",
                changed_target.as_object().unwrap(),
                "S2",
                "S3",
                "post_s1"
            )
            .is_err()
        );

        let mut changed_expectation = scenarios;
        changed_expectation["S3"]["repetitions"][0]["metrics"]["expected_outputs"] = json!(511);
        assert!(
            validate_paired_scan_targets(
                "new_wallet",
                changed_expectation.as_object().unwrap(),
                "S2",
                "S3",
                "post_s1"
            )
            .is_err()
        );
    }

    #[test]
    fn mode1_scan_contract_accepts_only_anchored_overshoot() {
        let mut metrics = json!({
            "max_height": 101,
            "H_tip_end": 100,
            "H_scan_cursor_hash": "completion",
            "H_scan_target_cursor_hash": "target",
            "H_tip_target_hash": "target",
            "scan_target_overshoot_allowed": true
        });

        assert!(scan_target_binding_valid("old_wallet", &metrics));
        assert!(!scan_target_binding_valid("new_wallet", &metrics));

        metrics["H_scan_target_cursor_hash"] = json!("wrong");
        assert!(!scan_target_binding_valid("old_wallet", &metrics));

        metrics["max_height"] = json!(100);
        metrics["H_scan_cursor_hash"] = json!("target");
        assert!(scan_target_binding_valid("new_wallet", &metrics));
    }

    #[test]
    fn s4_deadline_cleanup_rejects_late_acceptance_or_unreaped_work() {
        let document = json!({"config": {"S4_T_budget_secs": 900}});
        let arms = REFERENCE_S4_RAMP
            .into_iter()
            .map(|configured_batch| {
                json!({"configured_batch": configured_batch, "wall_ms": 900_000})
            })
            .collect::<Vec<_>>();
        let summaries = REFERENCE_S4_RAMP
            .into_iter()
            .map(|configured_batch| {
                json!({"configured_batch": configured_batch, "wall_ms": 900_000})
            })
            .collect::<Vec<_>>();
        let measurements = REFERENCE_S4_RAMP
            .into_iter()
            .map(|configured_batch| {
                json!({
                    "configured_batch": configured_batch,
                    "deadline_expired": true,
                    "deadline_offset_ms": 900_000,
                    "pending_tasks_at_deadline": 1,
                    "measured_wall_ms": 900_000
                })
            })
            .collect::<Vec<_>>();
        let cleanups = REFERENCE_S4_RAMP
            .into_iter()
            .map(|configured_batch| {
                json!({
                    "configured_batch": configured_batch,
                    "drain_ms": 1,
                    "cleanup_grace_ms": 0,
                    "cleanup_deadline_exhausted": false,
                    "abort_requested_tasks": 1,
                    "drained_tasks": 0,
                    "forced_shutdown_tasks": 1,
                    "completed_after_deadline_discarded_tasks": 0,
                    "post_deadline_join_errors": [],
                    "tasks_quiesced": true,
                    "unmatched_pending_attempts_after_drain": [{
                        "terminal_outcome": "timed_out",
                        "submission_state": "ambiguous"
                    }],
                    "late_results_accepted": false,
                    "actual_return_offset_ms": 900_001
                })
            })
            .collect::<Vec<_>>();
        let metrics = json!({
            "s4_deadline_measurements": measurements,
            "s4_post_deadline_cleanups": cleanups,
            "s4_actual_return_wall_ms": 4_500_005
        });
        let repetition = json!({"wall_ms": 4_500_000});
        validate_s4_deadline_accounting(
            &document,
            "old_wallet",
            &repetition,
            &metrics,
            &arms,
            &summaries,
        )
        .unwrap();

        let mut late_acceptance = metrics.clone();
        late_acceptance["s4_post_deadline_cleanups"][0]["late_results_accepted"] = json!(true);
        assert!(
            validate_s4_deadline_accounting(
                &document,
                "old_wallet",
                &repetition,
                &late_acceptance,
                &arms,
                &summaries,
            )
            .is_err()
        );

        let mut unreaped = metrics;
        unreaped["s4_post_deadline_cleanups"][0]["tasks_quiesced"] = json!(false);
        assert!(
            validate_s4_deadline_accounting(
                &document,
                "old_wallet",
                &repetition,
                &unreaped,
                &arms,
                &summaries,
            )
            .is_err()
        );
    }

    #[test]
    fn fee_disposition_is_outcome_consistent() {
        let cases = [
            ("confirmed", true, "tx", "confirmed_paid", Some(10u64)),
            ("rejected", false, "", "rejected", None),
            ("stalled", true, "batch", "proposed_unresolved", Some(10)),
            ("timed_out", true, "batch", "proposed_unresolved", Some(10)),
        ];
        for (outcome, accepted, identity, disposition, fee) in cases {
            let metrics = json!({
                "transaction_observations": [{
                    "terminal_outcome": outcome,
                    "api_accepted": accepted,
                    "transaction_id": (identity.starts_with("tx")).then_some(identity),
                    "batch_id": (identity.starts_with("batch")).then_some(identity),
                    "fee_microtari": fee,
                    "fee_unavailable_reason": fee.is_none().then_some("not constructed"),
                    "fee_disposition": disposition,
                    "error": (outcome != "confirmed").then_some("terminal outcome"),
                    "construction_timing_reason": "not exposed",
                    "construction_ms": null,
                    "confirmation_ms": (outcome == "confirmed").then_some(1),
                    "confirmation_timing_origin": (outcome == "confirmed").then_some("grpc_dispatch_to_independent_c_min"),
                    "mined_height": (outcome == "confirmed").then_some(1),
                    "tip_end_height": (outcome == "confirmed").then_some(2),
                    "output_commitments": (outcome == "confirmed").then_some(vec!["commitment"]),
                    "payment_output_count": (outcome == "confirmed" && identity.starts_with("tx")).then_some(1),
                    "change_output_count": (outcome == "confirmed" && identity.starts_with("tx")).then_some(1),
                    "total_output_count": (outcome == "confirmed" && identity.starts_with("tx")).then_some(2),
                    "recipients": ["recipient"]
                }],
                "batch_chain_identities": (identity.starts_with("batch") && outcome == "confirmed")
                    .then_some(json!([{"batch_id": identity, "transaction_id": "tx-1"}])),
                "constructed_output_shapes_by_tx_id":
                    (outcome == "confirmed" && identity.starts_with("tx")).then_some(json!({
                        "tx": {
                            "payment_output_count": 1,
                            "change_output_count": 1,
                            "total_output_count": 2
                        }
                    }))
            });
            let mode = if identity.starts_with("batch") {
                "payment_processor"
            } else {
                "old_wallet"
            };
            assert!(validate_transaction_observations(mode, "S1", &metrics).is_ok());
            for bad_disposition in [
                "confirmed_paid",
                "proposed_unresolved",
                "rejected",
                "unavailable",
            ] {
                if bad_disposition == disposition
                    || (matches!(outcome, "stalled" | "timed_out")
                        && bad_disposition == "unavailable")
                {
                    continue;
                }
                let mut mutated = metrics.clone();
                mutated["transaction_observations"][0]["fee_disposition"] = json!(bad_disposition);
                assert!(validate_transaction_observations("old_wallet", "S1", &mutated).is_err());
            }
        }
        let exposed_fee = json!({
            "transaction_observations": [{
                "terminal_outcome": "timed_out", "api_accepted": true,
                "transaction_id": "tx-1", "fee_microtari": 10,
                "fee_disposition": "unavailable", "error": "deadline",
                "construction_timing_reason": "not exposed", "construction_ms": null,
                "recipients": ["recipient"]
            }]
        });
        assert!(validate_transaction_observations("old_wallet", "S1", &exposed_fee).is_err());
    }

    #[test]
    fn incomplete_final_is_rejected() {
        let mut document = profile_document();
        document["profile_kind"] = json!("final");
        document["run_complete"] = json!(true);
        assert!(validate_document(&document, false).is_err());
    }

    #[test]
    fn summary_is_deterministic() {
        let document = profile_document();
        let first = render_summary(&document).unwrap();
        assert_eq!(first, render_summary(&document).unwrap());
        assert!(first.contains("Measurement commit: `"));
        assert!(first.contains("Export commit: `"));
        assert!(first.contains("Median ms (all)"));
        for column in [
            "API accepted",
            "Chain confirmed",
            "Rejected",
            "Stalled",
            "Timed out",
        ] {
            assert!(first.contains(column), "missing summary column {column}");
        }
    }

    #[test]
    fn summary_outcome_counts_come_from_structured_observations() {
        let cell = json!({
            "repetitions": [{"metrics": {"transaction_observations": [
                {"api_accepted": true, "terminal_outcome": "confirmed"},
                {"api_accepted": true, "terminal_outcome": "stalled"},
                {"api_accepted": false, "terminal_outcome": "rejected"},
                {"api_accepted": true, "terminal_outcome": "timed_out"}
            ]}}]
        });
        assert_eq!(summary_outcome_counts(&cell), (3, 1, 1, 1, 1));
    }

    #[test]
    fn null_checkpoint_manifests_are_allowed_but_submission_manifests_are_required() {
        let mut document = profile_document();
        document["provenance"]["measurement_build_manifest"] = Value::Null;
        assert!(validate_document(&document, false).is_ok());
        assert!(validate_document(&document, true).is_err());

        document["provenance"]["measurement_build_manifest"] = json!({
            "schema_version": 2,
            "artifacts": {"harness": {"source_revision": "", "sha256": "test"}}
        });
        assert!(validate_document(&document, false).is_err());
    }

    #[test]
    fn submission_harness_provenance_requires_exact_commit_and_clean_evidence() {
        let mut profile: ResultProfile = serde_json::from_value(profile_document()).unwrap();
        profile.provenance.measurement_commit = "11".repeat(20);
        profile.provenance.export_commit = "11".repeat(20);
        profile.provenance.measurement_worktree_clean = true;
        profile.provenance.export_worktree_clean = true;
        profile.provenance.measurement_build_worktree_clean = true;
        profile.provenance.export_build_worktree_clean = true;
        validate_submission_harness_provenance(&profile).unwrap();

        for invalid in ["not-a-commit".to_string(), "AA".repeat(20), "1".repeat(39)] {
            let mut bad_commit = profile.clone();
            bad_commit.provenance.measurement_commit = invalid.clone();
            bad_commit.provenance.export_commit = invalid;
            assert!(
                validate_submission_harness_provenance(&bad_commit)
                    .unwrap_err()
                    .to_string()
                    .contains("lowercase 40-character")
            );
        }

        let mut mismatched = profile.clone();
        mismatched.provenance.export_commit = "22".repeat(20);
        assert!(
            validate_submission_harness_provenance(&mismatched)
                .unwrap_err()
                .to_string()
                .contains("must be identical")
        );

        for measurement in [true, false] {
            let mut dirty = profile.clone();
            if measurement {
                dirty.provenance.measurement_worktree_clean = false;
            } else {
                dirty.provenance.export_worktree_clean = false;
            }
            assert!(
                validate_submission_harness_provenance(&dirty)
                    .unwrap_err()
                    .to_string()
                    .contains("clean-worktree evidence")
            );
        }

        let mut dirty_build = profile.clone();
        dirty_build.provenance.measurement_build_worktree_clean = false;
        assert!(
            validate_submission_harness_provenance(&dirty_build)
                .unwrap_err()
                .to_string()
                .contains("built from a clean worktree")
        );

        let mut replaced_executable = profile;
        replaced_executable.config.insert(
            "export_harness_executable_sha256".to_string(),
            json!("22".repeat(32)),
        );
        assert!(
            validate_submission_harness_provenance(&replaced_executable)
                .unwrap_err()
                .to_string()
                .contains("executable SHA-256 values must match")
        );
    }

    #[test]
    fn submission_build_manifests_must_be_identical() {
        let mut profile: ResultProfile = serde_json::from_value(profile_document()).unwrap();
        profile.provenance.export_build_manifest["artifacts"]["minotari"]["sha256"] =
            json!("11".repeat(32));
        let error = validate_submission_build_manifests(&profile)
            .unwrap_err()
            .to_string();
        assert!(error.contains("must be identical"));
    }

    #[test]
    fn submission_build_manifest_rejects_noncanonical_claims_and_sets() {
        let canonical =
            serde_json::to_value(crate::build_manifest::portable_manifest_fixture()).unwrap();
        let mut tampered_claim = canonical.clone();
        tampered_claim["sources"]["minotari_cli"]["patches"][0]["path"] =
            json!("patches/not-the-pinned-patch.patch");
        let mut extra_artifact = canonical.clone();
        let artifact_claim = extra_artifact["artifacts"]["minotari"].clone();
        extra_artifact["artifacts"]["unexpected"] = artifact_claim;
        let mut extra_source = canonical;
        let source_claim = extra_source["sources"]["minotari_cli"].clone();
        extra_source["sources"]["unexpected"] = source_claim;

        for tampered in [tampered_claim, extra_artifact, extra_source] {
            let mut profile: ResultProfile = serde_json::from_value(profile_document()).unwrap();
            profile.provenance.measurement_build_manifest = tampered.clone();
            profile.provenance.export_build_manifest = tampered.clone();
            profile
                .config
                .insert("build_manifest".to_string(), tampered);
            let error = validate_submission_build_manifests(&profile)
                .unwrap_err()
                .to_string();
            assert!(error.contains("does not match embedded canonical provenance"));
        }
    }

    #[test]
    fn submission_build_manifest_rejects_unknown_fields_and_bad_sha256() {
        let canonical =
            serde_json::to_value(crate::build_manifest::portable_manifest_fixture()).unwrap();
        let mut unknown_field = canonical.clone();
        unknown_field["unexpected_claim"] = json!(true);
        let mut uppercase_sha256 = canonical;
        uppercase_sha256["artifacts"]["minotari"]["sha256"] = json!("AA".repeat(32));

        for (tampered, expected) in [
            (unknown_field, "exact typed schema-v2 shape"),
            (
                uppercase_sha256,
                "does not match embedded canonical provenance",
            ),
        ] {
            let mut profile: ResultProfile = serde_json::from_value(profile_document()).unwrap();
            profile.provenance.measurement_build_manifest = tampered.clone();
            profile.provenance.export_build_manifest = tampered.clone();
            profile
                .config
                .insert("build_manifest".to_string(), tampered);
            let error = validate_submission_build_manifests(&profile)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn scan_history_validation_recomputes_commitment_evidence() {
        let document = json!({
            "modes": {
                "new_wallet": {
                    "scenarios": {
                        "S1": {"repetitions": [{"metrics": {"transaction_observations": [{
                            "transaction_id": "tx-1",
                            "terminal_outcome": "confirmed",
                            "output_commitments": ["commitment-1", "commitment-2"]
                        }]}}]}
                    }
                }
            }
        });
        let metrics = json!({
            "expected_output_commitments": {
                "tx-1": ["commitment-1", "commitment-2"]
            },
            "recovered_output_commitments": ["commitment-2"],
            "missing_history_output_tx_ids": []
        });
        validate_scan_history_evidence(&document, "new_wallet", "S2", &metrics).unwrap();

        let mut tampered = metrics;
        tampered["expected_output_commitments"] = json!({"tx-2": ["commitment-2"]});
        assert!(
            validate_scan_history_evidence(&document, "new_wallet", "S2", &tampered)
                .unwrap_err()
                .to_string()
                .contains("inconsistent")
        );
    }

    #[test]
    fn incomplete_s5_arms_never_create_comparison() {
        let config = Config::default();
        let mut profile = ResultProfile::new(&config, env_capture::capture());
        profile.refresh_computed_deltas();
        let comparisons = &profile.computed_deltas["s5_throughput"]["comparisons"];
        assert!(
            comparisons
                .as_object()
                .unwrap()
                .values()
                .all(Value::is_null)
        );
    }

    #[test]
    fn s5_requires_equal_configured_per_recipient_amounts() {
        let mut config = json!({
            "mode1_payment_amount": "1 T",
            "mode2_payment_amount": "1 T",
            "mode3_payment_amount": "1 T"
        });
        assert_eq!(
            reference_s5_per_recipient_amount(&config).unwrap(),
            1_000_000
        );

        config["mode3_payment_amount"] = json!("2 T");
        assert!(reference_s5_per_recipient_amount(&config).is_err());
    }

    #[test]
    fn s5_confirmed_amounts_use_per_recipient_and_batch_total_semantics() {
        let document = json!({
            "config": {
                "mode1_payment_amount": "1 T",
                "mode2_payment_amount": "1 T",
                "mode3_payment_amount": "1 T"
            }
        });
        let individual = json!({
            "transaction_observations": [{
                "terminal_outcome": "confirmed",
                "recipients": ["recipient-1"],
                "amount_microtari": 1_000_000
            }]
        });
        validate_s5_observation_amounts(&document, "old_wallet", &individual).unwrap();
        validate_s5_observation_amounts(&document, "new_wallet", &individual).unwrap();

        let recipients = (1..=10)
            .map(|index| format!("recipient-{index}"))
            .collect::<Vec<_>>();
        let batch = json!({
            "transaction_observations": [{
                "terminal_outcome": "confirmed",
                "recipients": recipients,
                "amount_microtari": 10_000_000
            }]
        });
        validate_s5_observation_amounts(&document, "payment_processor", &batch).unwrap();

        for (mode, mut tampered) in [
            ("old_wallet", individual.clone()),
            ("new_wallet", individual),
            ("payment_processor", batch),
        ] {
            tampered["transaction_observations"][0]["amount_microtari"] = json!(999);
            assert!(
                validate_s5_observation_amounts(&document, mode, &tampered).is_err(),
                "tampered {mode} S5 amount unexpectedly passed"
            );
        }
    }

    #[test]
    fn s5_self_send_arms_are_explicitly_unavailable() {
        let arms = json!({
            "old_wallet": {"self_send": {"complete": false, "unavailable_reason": "unsupported"}},
            "new_wallet": {"self_send": {"complete": false, "unavailable_reason": "unsupported"}},
            "payment_processor": {"self_send": {"complete": false, "unavailable_reason": "unsupported"}}
        });
        let document = json!({
            "computed_deltas": {
                "s5_throughput": {
                    "arms": arms,
                    "comparisons": {
                        "new_wallet_individual_over_payment_processor_batch": null,
                        "old_wallet_individual_over_payment_processor_batch": null,
                        "new_wallet_individual_fee_per_recipient_over_payment_processor_batch": null,
                        "old_wallet_individual_fee_per_recipient_over_payment_processor_batch": null
                    }
                }
            }
        });
        validate_s5_comparisons(&document).unwrap();
    }

    #[test]
    fn outcome_status_enum_remains_stable() {
        assert_eq!(
            serde_json::to_value(crate::result_profile::OutcomeStatus::Success).unwrap(),
            json!("success")
        );
    }

    #[test]
    fn schema_accepts_realistic_s1_s4_s5_transaction_observations() {
        let mut document = profile_document();
        let observation = json!({
            "transaction_id": "tx-1",
            "attempt_index": 1,
            "batch_index": null,
            "submit_offset_ms": 2,
            "construction_complete_offset_ms": 7,
            "construction_ms": 5,
            "submission_ms": 3,
            "submission_started": true,
            "mempool_available": true,
            "mempool_reason": null,
            "confirmation_ms": 40,
            "confirmation_timing_reason": null,
            "amount_microtari": 1000000,
            "fee_microtari": 700,
            "terminal_outcome": "confirmed",
            "error": null,
            "mined_height": 100,
            "confirmations": 3,
            "min_confirmations": 3,
            "tip_start_height": 99,
            "tip_end_height": 103,
            "input_count": 1,
            "total_output_count": 2,
            "payment_output_count": 1,
            "change_output_count": 1,
            "output_commitments": ["commitment"],
            "configured_batch": 8
        });
        for mode in ["old_wallet", "new_wallet", "payment_processor"] {
            for scenario in ["S1", "S4", "S5"] {
                document["modes"][mode]["scenarios"][scenario]["repetitions"] = json!([{
                    "run": 1,
                    "execution_status": "completed",
                    "outcome_status": "success",
                    "wall_ms": 50,
                    "success_count": 1,
                    "failure_count": 0,
                    "fee_microtari": 700,
                    "error": null,
                    "metrics": {"transaction_observations": [observation.clone()]}
                }]);
            }
        }
        validate_schema(&document).unwrap();

        assert!(
            schema_document()["$defs"]["transaction_observation"]["required"]
                .as_array()
                .expect("transaction observation required fields")
                .iter()
                .any(|field| field == "submission_started")
        );
        let mut missing_started = document;
        missing_started["modes"]["old_wallet"]["scenarios"]["S1"]["repetitions"][0]["metrics"]
            ["transaction_observations"][0]
            .as_object_mut()
            .expect("observation object")
            .remove("submission_started");
        assert!(validate_schema(&missing_started).is_err());
    }

    #[test]
    fn validation_recomputes_and_rejects_tampered_deltas() {
        let mut document = profile_document();
        document["computed_deltas"]["scan_deltas"]["old_wallet"]["t_scan_s6_over_b0"] =
            json!(999.0);
        let error = validate_document(&document, false).unwrap_err().to_string();
        assert!(error.contains("computed_deltas"));
    }

    #[test]
    fn realistic_submission_accepts_honest_wallet_failure_and_blocked_dependents() {
        let mut config = Config::default();
        config.network.base_node_http_url = "http://127.0.0.1:18142".to_string();
        config.network.authority_http_url = "https://rpc.esmeralda.tari.com".to_string();
        config.network.mode1_base_node_service_peer =
            Some("abc::/ip4/127.0.0.1/tcp/18189".to_string());
        config.benchmark.mode1_live_topology = true;
        config.benchmark.mode2_live_scenarios = true;
        config.benchmark.mode3_live_topology = true;
        config.benchmark.live_fresh_scan_cells = true;
        let funding = crate::config::FundingRecord {
            amount: "10000 T".to_string(),
            tx_id: "funding-tx".to_string(),
            height: 101,
            birthday: Some(3),
            birthday_height: Some(95),
            birthday_offset_days: Some(2),
            birthday_start_height: Some(90),
            construction_ms: Some(1),
            broadcast_to_mempool_ms: Some(2),
            broadcast_to_mempool_unavailable_reason: None,
            broadcast_to_confirmed_at_c_min_ms: Some(3),
            tip_height_at_broadcast: Some(101),
            tip_height_at_confirmation: Some(104),
            shared_funding_fee_microtari: Some(700),
            funding_fee_attribution: Some(
                "external_source_shared_not_deducted_from_mode_balance".to_string(),
            ),
        };
        config.funding.old_wallet = Some(funding.clone());
        config.funding.new_wallet = Some(funding.clone());
        config.funding.payment_processor = Some(funding);
        let mut profile = ResultProfile::new(
            &config,
            env_capture::capture_for_network(
                &config.network.base_node_http_url,
                &config.network.authority_http_url,
                config.network.mode1_base_node_service_peer.as_deref(),
            ),
        );
        let manifest =
            serde_json::to_value(crate::build_manifest::portable_manifest_fixture()).unwrap();
        profile
            .config
            .insert("build_manifest".to_string(), manifest.clone());
        profile.provenance.measurement_build_manifest = manifest.clone();
        profile.provenance.export_build_manifest = manifest;
        profile.config.insert(
            "seed_fingerprints".to_string(),
            json!({
                "old_wallet": "a",
                "new_wallet": "b",
                "payment_processor": "c",
                "recipient_sink": "d"
            }),
        );
        profile.completed_stages = REQUIRED_STAGES.iter().map(ToString::to_string).collect();
        profile.base_node.tip_start_height = Some(100);
        profile.base_node.tip_start_hash = Some("aa".repeat(32));
        profile.base_node.tip_end_height = Some(103);
        profile.base_node.tip_end_hash = Some("bb".repeat(32));
        profile.base_node.authority_tip_start_height = Some(100);
        profile.base_node.authority_tip_start_hash = Some("aa".repeat(32));
        profile.base_node.authority_tip_end_height = Some(103);
        profile.base_node.authority_tip_end_hash = Some("bb".repeat(32));
        profile.base_node.pruning_horizon = Some(0);
        profile.base_node.pruned_height = Some(0);
        profile.base_node.is_synced = Some(true);

        for mode in ModeName::ALL {
            let mut mode_profile = empty_mode_profile(mode, Some(format!("{mode:?}-address")));
            mode_profile
                .scenarios
                .get_mut("B0")
                .unwrap()
                .record_repetition(crate::result_profile::Repetition {
                    run: 1,
                    status: CellStatus::Ok,
                    wall_ms: Some(10),
                    success_count: 1,
                    failure_count: 0,
                    fee_microtari: Some(0),
                    error: None,
                    metrics: Some(json!({
                        "T_scan_ms": 10, "blocks_per_sec": 10.0,
                        "H_tip_start": 100, "H_tip_end": 100,
                        "H_tip_target_hash": "aa", "H_scan_target_cursor_hash": "aa",
                        "H_scan_cursor_hash": "aa",
                        "H_tip_completion": 101,
                        "H_tip_completion_hash": "bb", "birthday": 0,
                        "expected_outputs": 0, "outputs_match_expected": true,
                        "expected_available_microtari": 0,
                        "expected_history_transactions": 0,
                        "history_count_matches_expected": true,
                        "balance_matches_expected": true,
                        "detected_outputs": 0, "spendable_outputs": 0,
                        "available_microtari": 0, "history_transactions": 0,
                        "max_height": 100, "tip_lag_blocks": 0,
                        "tip_lag_tolerance_blocks": 0, "scan_reached_tip": true,
                        "scan_invocations": 1,
                        "scan_no_progress_attempts": 0,
                        "scan_stopped_without_progress": false,
                        "scan_last_more_blocks": false,
                        "scan_stop_reason": null,
                        "scan_timed_out": false,
                        "scan_timeout_ms": null,
                        "scan_error": null,
                        "scan_overshoot_blocks": 0,
                        "scan_target_overshoot_allowed": false,
                        "peak_rss_bytes": 1, "peak_cpu_percent": 1.0,
                        "balance_delta_microtari": 0
                    })),
                });
            mode_profile
                .scenarios
                .get_mut("S0")
                .unwrap()
                .record_repetition(crate::result_profile::Repetition {
                    run: 1,
                    status: CellStatus::Ok,
                    wall_ms: Some(1),
                    success_count: 1,
                    failure_count: 0,
                    fee_microtari: Some(0),
                    error: None,
                    metrics: Some(json!({
                        "verification_source": "wallet_state_observed",
                        "balance_delta_microtari": 0,
                        "expected_spendable_count": 1,
                        "observed_spendable_count": 1,
                        "expected_available_microtari": 10000000000u64,
                        "available_microtari": 10000000000u64,
                        "pending_outputs": 0,
                        "locked_outputs": 0,
                        "invalid_outputs": 0,
                        "unknown_outputs": 0,
                        "wallet_state_complete": true,
                        "spendable_count_matches_expected": true,
                        "balance_matches_expected": true,
                        "funding_observation": {
                            "tx_id": "funding-tx",
                            "mined_height": 101,
                            "birthday": 3,
                            "H_birth": 95,
                            "birthday_height": 95,
                            "birthday_offset_days": 2,
                            "birthday_start_height": 90,
                            "shared_funding_fee_microtari": 700
                        }
                    })),
                });
            mode_profile
                .scenarios
                .get_mut("S1")
                .unwrap()
                .record_repetition(crate::result_profile::Repetition {
                    run: 1,
                    status: CellStatus::Failed,
                    wall_ms: Some(5),
                    success_count: 0,
                    failure_count: 1,
                    fee_microtari: Some(0),
                    error: Some("wallet rejected construction".to_string()),
                    metrics: Some(json!({
                        "balance_reconciliation_unavailable_reason": "wallet rejected construction",
                        "scenario_tip_start_height": 100,
                        "scenario_tip_end_height": 100,
                        "balance_before": {"total": 1, "available": 1, "locked": 0, "unconfirmed": 0, "immature": 0, "pending_incoming": 0, "pending_outgoing": 0, "timelocked": 0},
                        "balance_after": {"total": 1, "available": 1, "locked": 0, "unconfirmed": 0, "immature": 0, "pending_incoming": 0, "pending_outgoing": 0, "timelocked": 0},
                        "outcome_counts": {
                            "attempted": 1, "accepted": 0, "confirmed": 0,
                            "rejected": 1, "stalled": 0, "timed_out": 0
                        },
                        "transaction_observations": [{
                            "transaction_id": null, "attempt_index": 1, "batch_index": null,
                            "batch_id": null,
                            "submit_offset_ms": 0, "construction_complete_offset_ms": null,
                            "broadcast_start_offset_ms": null,
                            "construction_ms": null, "submission_ms": null,
                            "construction_timing_origin": null,
                            "construction_timing_reason": "construction failed",
                            "submission_timing_origin": null,
                            "submission_started": true,
                            "mempool_available": null, "mempool_reason": "not submitted",
                            "confirmation_ms": null, "confirmation_timing_origin": null,
                            "confirmation_timing_reason": "not submitted",
                            "fee_microtari": null, "fee_unavailable_reason": "not constructed", "fee_disposition": "rejected",
                            "recipient": null, "recipients": [], "api_accepted": false,
                            "api_error": "wallet rejected construction", "terminal_outcome": "rejected",
                            "error": "wallet rejected construction", "mined_height": null,
                            "tip_start_height": 100, "tip_end_height": null,
                            "input_count": null, "total_output_count": null,
                            "payment_output_count": null, "change_output_count": null,
                            "output_commitments": [], "configured_batch": null
                        }]
                    })),
                });
            for scenario in ["S2", "S3", "S4", "S5", "S6", "S7"] {
                mode_profile
                    .scenarios
                    .get_mut(scenario)
                    .unwrap()
                    .record_repetition(crate::result_profile::Repetition {
                        run: 1,
                        status: CellStatus::BlockedPrerequisite,
                        wall_ms: None,
                        success_count: 0,
                        failure_count: 0,
                        fee_microtari: None,
                        error: None,
                        metrics: Some(json!({
                            "blocked_prerequisite": true,
                            "balance_reconciliation_unavailable_reason": "blocked by S1"
                        })),
                    });
            }
            profile
                .modes
                .insert(mode.as_str().to_string(), mode_profile);
        }
        profile.mark_final();
        profile.provenance.measurement_commit = "11".repeat(20);
        profile.provenance.export_commit = "11".repeat(20);
        profile.provenance.measurement_worktree_clean = true;
        profile.provenance.export_worktree_clean = true;
        profile.provenance.measurement_build_worktree_clean = true;
        profile.provenance.export_build_worktree_clean = true;
        profile.refresh_computed_deltas();
        profile.validate_submission().unwrap();

        let mut b0_failure = serde_json::to_value(&profile).unwrap();
        for mode in ["new_wallet", "payment_processor"] {
            let cell = &mut b0_failure["modes"][mode]["scenarios"]["B0"];
            cell["execution_status"] = json!("completed");
            cell["outcome_status"] = json!("failure");
            let run = &mut cell["repetitions"][0];
            run["execution_status"] = json!("completed");
            run["outcome_status"] = json!("failure");
            run["success_count"] = json!(0);
            run["failure_count"] = json!(1);
            run["error"] = json!("scanner returned 17 blocks below fixed target");
            let metrics = &mut run["metrics"];
            metrics["max_height"] = json!(83);
            metrics["H_scan_target_cursor_hash"] = Value::Null;
            metrics["H_scan_cursor_hash"] = json!("partial");
            metrics["tip_lag_blocks"] = json!(17);
            metrics["scan_reached_tip"] = json!(false);
            metrics["scan_stop_reason"] = json!("scanner_returned_below_fixed_target");
        }
        let mut b0_failure_profile: ResultProfile = serde_json::from_value(b0_failure).unwrap();
        b0_failure_profile.refresh_computed_deltas();
        let b0_failure = serde_json::to_value(&b0_failure_profile).unwrap();
        validate_document(&b0_failure, true).unwrap();

        for field in [
            "scan_invocations",
            "tip_lag_blocks",
            "H_scan_target_cursor_hash",
            "scan_last_more_blocks",
        ] {
            let mut tampered = b0_failure.clone();
            tampered["modes"]["new_wallet"]["scenarios"]["B0"]["repetitions"][0]["metrics"]
                [field] = match field {
                "scan_invocations" => json!(2),
                "tip_lag_blocks" => json!(16),
                "H_scan_target_cursor_hash" => json!("false-target-claim"),
                _ => Value::Null,
            };
            assert!(validate_document(&tampered, true).is_err(), "{field}");
        }

        for field in [
            "scan_timed_out",
            "scan_target_overshoot_allowed",
            "scan_stopped_without_progress",
            "scan_no_progress_attempts",
        ] {
            let mut tampered = serde_json::to_value(&profile).unwrap();
            tampered["modes"]["old_wallet"]["scenarios"]["B0"]["repetitions"][0]["metrics"]
                [field] = if field == "scan_no_progress_attempts" {
                json!(1)
            } else {
                json!(true)
            };
            assert!(validate_document(&tampered, true).is_err(), "{field}");
        }

        let mut mismatched_b0_status = b0_failure.clone();
        mismatched_b0_status["modes"]["new_wallet"]["scenarios"]["B0"]["outcome_status"] =
            json!("success");
        assert!(validate_document(&mismatched_b0_status, true).is_err());

        let mut failed_s0 = serde_json::to_value(&profile).unwrap();
        failed_s0["modes"]["new_wallet"]["scenarios"]["S0"]["outcome_status"] = json!("failure");
        failed_s0["modes"]["new_wallet"]["scenarios"]["S0"]["repetitions"][0]["outcome_status"] =
            json!("failure");
        let mut failed_s0_profile: ResultProfile = serde_json::from_value(failed_s0).unwrap();
        failed_s0_profile.refresh_computed_deltas();
        assert!(
            validate_document(&serde_json::to_value(failed_s0_profile).unwrap(), true).is_err()
        );

        let mut missing_disk = profile.clone();
        missing_disk.environment.disk_kind = None;
        assert!(
            missing_disk
                .validate_submission()
                .unwrap_err()
                .to_string()
                .contains("disk kind and device")
        );

        for pruned_height in [None, Some(1)] {
            let mut non_archival = profile.clone();
            non_archival.base_node.pruned_height = pruned_height;
            assert!(
                non_archival
                    .validate_submission()
                    .unwrap_err()
                    .to_string()
                    .contains("pruned_height")
            );
        }

        for field in [
            "birthday",
            "birthday_height",
            "birthday_offset_days",
            "birthday_start_height",
        ] {
            let mut missing_truth = serde_json::to_value(&profile).unwrap();
            missing_truth["funding"]["old_wallet"][field] = Value::Null;
            assert!(validate_document(&missing_truth, true).is_err(), "{field}");
        }

        let mut invalid_birthday_order = serde_json::to_value(&profile).unwrap();
        invalid_birthday_order["funding"]["old_wallet"]["birthday_start_height"] = json!(96);
        assert!(validate_document(&invalid_birthday_order, true).is_err());

        let mut mismatched_h_birth = serde_json::to_value(&profile).unwrap();
        mismatched_h_birth["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0]["metrics"]
            ["funding_observation"]["H_birth"] = json!(94);
        assert!(validate_document(&mismatched_h_birth, true).is_err());

        let mut repeated = serde_json::to_value(&profile).unwrap();
        let second_s0 =
            repeated["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0].clone();
        repeated["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"]
            .as_array_mut()
            .unwrap()
            .push(second_s0);
        repeated["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][1]["metrics"]["available_microtari"] =
            json!(999);
        assert!(validate_document(&repeated, true).is_err());

        for field in [
            "pending_outputs",
            "locked_outputs",
            "invalid_outputs",
            "unknown_outputs",
        ] {
            let mut mutated = serde_json::to_value(&profile).unwrap();
            mutated["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0]["metrics"][field] =
                json!(1);
            assert!(validate_document(&mutated, true).is_err(), "{field}");
        }

        let mut tampered_fee = serde_json::to_value(&profile).unwrap();
        tampered_fee["modes"]["old_wallet"]["scenarios"]["S1"]["repetitions"][0]["fee_microtari"] =
            json!(1);
        assert!(validate_document(&tampered_fee, true).is_err());

        let mut mismatched = serde_json::to_value(&profile).unwrap();
        mismatched["modes"]["new_wallet"]["scenarios"]["B0"]["repetitions"][0]["metrics"]["H_tip_target_hash"] =
            json!("different");
        mismatched["modes"]["new_wallet"]["scenarios"]["B0"]["repetitions"][0]["metrics"]["H_scan_cursor_hash"] =
            json!("different");
        mismatched["modes"]["new_wallet"]["scenarios"]["B0"]["repetitions"][0]["metrics"]["H_scan_target_cursor_hash"] =
            json!("different");
        let error = validate_document(&mismatched, true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("do not share one target"));
    }
}
