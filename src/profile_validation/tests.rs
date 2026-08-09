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
        extra_artifact["provenance"]["measurement_build_manifest"]["artifacts"]["minotari"].clone();
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
    validate_balance_reconciliation(metrics.as_object().unwrap(), "test", Some(100), 600).unwrap();
    let mut mutated = metrics.clone();
    mutated["balance_reconciliation"]["delta_microtari"] = json!(1);
    assert!(
        validate_balance_reconciliation(mutated.as_object().unwrap(), "test", Some(100), 600,)
            .is_err()
    );
    let mut underflow = metrics.clone();
    underflow["balance_before_microtari"] = json!(50);
    assert!(
        validate_balance_reconciliation(underflow.as_object().unwrap(), "test", Some(100), 600,)
            .is_err()
    );
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
            validate_transaction_observations("payment_processor", "S1", &wrong_started).is_err(),
            "tampered submission_started unexpectedly passed for {failure_class}"
        );

        let mut wrong_origin = metrics;
        wrong_origin["transaction_observations"][0]["submission_timing_origin"] =
            json!("payment_processor_api_acceptance");
        assert!(
            validate_transaction_observations("payment_processor", "S1", &wrong_origin).is_err(),
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
            fake_submission["extra"]["preflight_failures"][0]["submission_started"] = json!(true);
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
        validate_transaction_observations("old_wallet", "S1", &mislabeled_construction).is_err()
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
        .map(|configured_batch| json!({"configured_batch": configured_batch, "wall_ms": 900_000}))
        .collect::<Vec<_>>();
    let summaries = REFERENCE_S4_RAMP
        .into_iter()
        .map(|configured_batch| json!({"configured_batch": configured_batch, "wall_ms": 900_000}))
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
                || (matches!(outcome, "stalled" | "timed_out") && bad_disposition == "unavailable")
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
    assert!(first.contains("## Findings"));
    assert!(first.contains("measurement-coverage"));
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
    document["computed_deltas"]["scan_deltas"]["old_wallet"]["t_scan_s6_over_b0"] = json!(999.0);
    let error = validate_document(&document, false).unwrap_err().to_string();
    assert!(error.contains("computed_deltas"));
}

#[test]
fn realistic_submission_accepts_honest_wallet_failure_and_blocked_dependents() {
    let mut config = Config::default();
    config.network.base_node_http_url = "http://127.0.0.1:18142".to_string();
    config.network.authority_http_url = "https://rpc.esmeralda.tari.com".to_string();
    config.network.mode1_base_node_service_peer = Some("abc::/ip4/127.0.0.1/tcp/18189".to_string());
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

    let mut continued_scan = serde_json::to_value(&profile).unwrap();
    for mode in ["old_wallet", "new_wallet", "payment_processor"] {
        continued_scan["modes"][mode]["scenarios"]["B0"]["repetitions"][0]["metrics"]["scan_invocations"] =
            json!(2);
    }
    validate_document(&continued_scan, true).unwrap();

    let mut missing_findings = serde_json::to_value(&profile).unwrap();
    missing_findings["findings"] = json!([]);
    assert!(validate_document(&missing_findings, true).is_err());

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
        tampered["modes"]["new_wallet"]["scenarios"]["B0"]["repetitions"][0]["metrics"][field] =
            match field {
                "scan_invocations" => json!(0),
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
        tampered["modes"]["old_wallet"]["scenarios"]["B0"]["repetitions"][0]["metrics"][field] =
            if field == "scan_no_progress_attempts" {
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
    let s0 = &mut failed_s0["modes"]["new_wallet"]["scenarios"]["S0"];
    s0["outcome_status"] = json!("failure");
    let run = &mut s0["repetitions"][0];
    run["outcome_status"] = json!("failure");
    run["success_count"] = json!(0);
    run["failure_count"] = json!(1);
    run["error"] = json!("wallet scanner returned below the fixed S0 target");
    run["metrics"] = json!({
        "balance_reconciliation_unavailable_reason":
            "wallet scanner failed before final wallet balance could be observed"
    });
    for scenario in ["S1", "S2", "S3", "S4", "S5", "S6", "S7"] {
        let cell = &mut failed_s0["modes"]["new_wallet"]["scenarios"][scenario];
        cell["execution_status"] = json!("blocked_prerequisite");
        cell["outcome_status"] = json!("unavailable");
        cell["repetitions"] = json!([{
            "run": 1,
            "execution_status": "blocked_prerequisite",
            "outcome_status": "unavailable",
            "wall_ms": null,
            "success_count": 0,
            "failure_count": 0,
            "fee_microtari": null,
            "error": null,
            "metrics": {
                "blocked_prerequisite": true,
                "prerequisite": "S0",
                "balance_reconciliation_unavailable_reason": "blocked by S0"
            }
        }]);
    }
    let mut failed_s0_profile: ResultProfile = serde_json::from_value(failed_s0).unwrap();
    failed_s0_profile.refresh_computed_deltas();
    let failed_s0 = serde_json::to_value(failed_s0_profile).unwrap();
    validate_document(&failed_s0, true).unwrap();

    let mut unblocked_after_failed_s0 = failed_s0.clone();
    unblocked_after_failed_s0["modes"]["new_wallet"]["scenarios"]["S1"] =
        serde_json::to_value(&profile.modes["new_wallet"].scenarios["S1"]).unwrap();
    assert!(validate_document(&unblocked_after_failed_s0, true).is_err());

    let mut missing_s0_error = failed_s0;
    missing_s0_error["modes"]["new_wallet"]["scenarios"]["S0"]["repetitions"][0]["error"] =
        Value::Null;
    assert!(validate_document(&missing_s0_error, true).is_err());

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
    mismatched_h_birth["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0]["metrics"]["funding_observation"]
        ["H_birth"] = json!(94);
    assert!(validate_document(&mismatched_h_birth, true).is_err());

    let mut repeated = serde_json::to_value(&profile).unwrap();
    let second_s0 = repeated["modes"]["old_wallet"]["scenarios"]["S0"]["repetitions"][0].clone();
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
