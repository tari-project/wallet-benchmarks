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
const REFERENCE_S1_TRANSACTION_COUNT: usize = reference_s1_transaction_count();
const REFERENCE_S4_RAMP: [u64; 5] = [8, 16, 32, 64, 128];

const fn reference_s1_transaction_count() -> usize {
    let mut index = 0;
    let mut total = 0usize;
    while index < REFERENCE_S1_ROUNDS.len() {
        total += REFERENCE_S1_ROUNDS[index].1 as usize;
        index += 1;
    }
    total
}

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
                    "id": {"type": "string", "minLength": 1}, "title": {"type": "string", "minLength": 1},
                    "status": {"type": "string", "minLength": 1}, "recommendation": {"type": "string", "minLength": 1}
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
        validate_submission_findings(profile)?;
        validate_reference_configuration(profile, document)?;
    }
    Ok(())
}

fn validate_submission_findings(profile: &ResultProfile) -> anyhow::Result<()> {
    if profile.findings.is_empty() {
        bail!("submission profile must contain deterministic comparative findings");
    }
    let mut ids = BTreeSet::new();
    for finding in &profile.findings {
        if !ids.insert(finding.id.as_str()) {
            bail!(
                "submission profile contains duplicate finding id {}",
                finding.id
            );
        }
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
        validate_s0_dependency_chain(mode, scenarios)?;
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

fn validate_s0_dependency_chain(
    mode: &str,
    scenarios: &serde_json::Map<String, Value>,
) -> anyhow::Result<()> {
    let s0 = &scenarios["S0"];
    let runs = s0["repetitions"].as_array().expect("schema checked");
    if s0["execution_status"] != "completed"
        || !matches!(s0["outcome_status"].as_str(), Some("success" | "failure"))
        || runs.len() != 1
        || runs[0]["execution_status"] != "completed"
        || runs[0]["outcome_status"] != s0["outcome_status"]
    {
        bail!("submission requires one completed S0 wallet outcome for {mode}");
    }
    let run = &runs[0];
    match s0["outcome_status"].as_str() {
        Some("success") => {
            if run["success_count"] != 1 || run["failure_count"] != 0 || !run["error"].is_null() {
                bail!("submission successful S0 aggregates are incoherent for {mode}");
            }
        }
        Some("failure") => {
            if run["success_count"] != 0
                || run["failure_count"] != 1
                || run["error"].as_str().is_none_or(str::is_empty)
                || run["metrics"]["balance_reconciliation_unavailable_reason"]
                    .as_str()
                    .is_none_or(str::is_empty)
            {
                bail!("submission failed S0 evidence is incomplete for {mode}");
            }
            for scenario in ["S1", "S2", "S3", "S4", "S5", "S6", "S7"] {
                let cell = &scenarios[scenario];
                let repetitions = cell["repetitions"].as_array().expect("schema checked");
                if cell["execution_status"] != "blocked_prerequisite"
                    || cell["outcome_status"] != "unavailable"
                    || repetitions.len() != 1
                    || repetitions[0]["execution_status"] != "blocked_prerequisite"
                    || repetitions[0]["outcome_status"] != "unavailable"
                    || repetitions[0]["metrics"]["blocked_prerequisite"] != true
                    || repetitions[0]["metrics"]["prerequisite"] != "S0"
                {
                    bail!("submission {mode}/{scenario} must be blocked by failed S0");
                }
            }
        }
        _ => unreachable!("checked above"),
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
            || b0["scan_invocations"]
                .as_u64()
                .is_none_or(|count| count == 0)
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
                .is_none_or(|observations| observations.len() != REFERENCE_S1_TRANSACTION_COUNT)
            {
                bail!(
                    "submission {mode}/S1 must contain one observation for each of {REFERENCE_S1_TRANSACTION_COUNT} transactions"
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
        } else if scenarios["S1"]["execution_status"] == "completed" {
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
    output.push_str("\n## Findings\n\n");
    if let Some(findings) = document["findings"].as_array() {
        for finding in findings {
            output.push_str(&format!(
                "- **{}** (`{}`): {} Recommendation: {}\n",
                markdown_text(&finding["title"]),
                markdown_text(&finding["status"]),
                markdown_text(&finding["id"]),
                markdown_text(&finding["recommendation"])
            ));
        }
    }
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
mod tests;
