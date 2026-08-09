use super::*;
use crate::{
    config::Config,
    modes::ModeName,
    result_profile::{ResultProfile, empty_mode_profile},
};

#[test]
fn strict_s0_rejects_incomplete_or_dirty_wallet_state() {
    let incomplete = strict_s0_status(100, 100, Some(1), None);
    assert_eq!(incomplete.0, CellStatus::Failed);
    let dirty = strict_s0_status(
        100,
        100,
        Some(1),
        Some(serde_json::json!({
            "pending_outputs": 1,
            "locked_outputs": 0,
            "invalid_outputs": 0,
            "unknown_outputs": 0
        })),
    );
    assert_eq!(dirty.0, CellStatus::Failed);
    let clean = strict_s0_status(
        100,
        100,
        Some(1),
        Some(serde_json::json!({
            "pending_outputs": 0,
            "locked_outputs": 0,
            "invalid_outputs": 0,
            "unknown_outputs": 0
        })),
    );
    assert_eq!(clean.0, CellStatus::Ok);
}

#[test]
fn balance_reconciliation_marks_underflow_unavailable() {
    let mut metrics = serde_json::Map::new();
    add_balance_reconciliation_metrics(&mut metrics, Some(10), Some(0), 11, 0);
    assert!(metrics.get("balance_reconciliation").is_none());
    assert!(
        metrics
            .get("balance_reconciliation_unavailable_reason")
            .and_then(serde_json::Value::as_str)
            .is_some()
    );
}

#[test]
fn incomplete_total_components_preserve_available_reconciliation() {
    let mut metrics = serde_json::Map::new();
    add_balance_reconciliation_metrics(&mut metrics, Some(100), Some(90), 5, 5);
    add_balance_component_metrics(
        &mut metrics,
        Some(serde_json::json!({"total": 100})),
        Some(serde_json::json!({"available": 90})),
        5,
        5,
    );
    assert_eq!(
        metrics["balance_reconciliation"]["balance_domain"],
        "available"
    );
    assert!(
        metrics
            .get("balance_components_unavailable_reason")
            .and_then(serde_json::Value::as_str)
            .is_some()
    );
    assert!(
        metrics
            .get("balance_reconciliation_unavailable_reason")
            .is_none()
    );
}

#[test]
fn mode1_operator_recovery_resumes_an_existing_database() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("console_wallet.db");
    assert!(mode1_recovery_required(&db));
    std::fs::write(&db, []).unwrap();
    assert!(!mode1_recovery_required(&db));
}

#[test]
fn s1_round_plan_reaches_512_without_cap() {
    let config = Config::default();
    let rounds = s1_round_plan(&config, 0);
    assert_eq!(rounds.len(), 7);
    assert_eq!(rounds[0].tx_count, 1);
    assert_eq!(rounds[5].target_utxos_after, 64);
    assert_eq!(rounds[6].tx_count, 64);
    assert_eq!(rounds[6].outputs_per_tx, 8);
    assert_eq!(rounds[6].target_utxos_after, 512);
}

#[test]
fn s1_round_plan_honors_cap_without_inventing_rounds() {
    let config = Config::default();
    let rounds = s1_round_plan(&config, 3);
    assert_eq!(rounds.len(), 2);
    assert_eq!(rounds[0].tx_count, 1);
    assert_eq!(rounds[1].tx_count, 2);
    assert!(!rounds[1].fanout);
}

#[test]
fn serialization_gap_uses_sorted_completion_times() {
    assert_eq!(max_serialization_gap_ms(vec![30, 10, 50, 35]), Some(20));
    assert_eq!(max_serialization_gap_ms(vec![10]), None);
}

#[test]
fn s4_serialization_gap_uses_only_genuine_mode_specific_construction_events() {
    let batch_summaries = serde_json::json!([{
        "configured_batch": 4,
        "wall_ms": 900_000
    }]);
    let pp_observations = vec![
        serde_json::json!({
            "configured_batch": 4,
            "api_accepted": true,
            "terminal_outcome": "stalled",
            "construction_complete_offset_ms": 100,
            "construction_timing_origin": "payment_processor_db_unsigned_tx_first_observation",
            "recipients": ["a"]
        }),
        serde_json::json!({
            "configured_batch": 4,
            "api_accepted": true,
            "terminal_outcome": "stalled",
            "construction_complete_offset_ms": 145,
            "construction_timing_origin": "payment_processor_db_unsigned_tx_first_observation",
            "recipients": ["b"]
        }),
    ];

    let pp_arms = s4_arms_metrics("payment_processor", &pp_observations, &batch_summaries);
    assert_eq!(pp_arms[0]["max_serialization_gap_ms"], 45);
    assert!(pp_arms[0]["serialization_timing_reason"].is_null());

    let old_arms = s4_arms_metrics("old_wallet", &pp_observations, &batch_summaries);
    assert!(old_arms[0]["max_serialization_gap_ms"].is_null());
    assert!(
        old_arms[0]["serialization_timing_reason"]
            .as_str()
            .unwrap()
            .contains("does not expose a genuine")
    );
}

#[test]
fn double_selection_rejections_classifies_wallet_lock_errors() {
    let errors = vec![
        "Funds are pending. Available: 0".to_string(),
        "Funds are still pending. Unable to fulfil transaction right now.".to_string(),
        "duplicate input detected".to_string(),
        "plain network timeout".to_string(),
    ];
    assert_eq!(double_selection_rejections(&errors), 3);
}

#[test]
fn mode1_summary_backfills_missing_verified_fee_total() {
    let mut summary = Mode1TransferSummary {
        fee_microtari: 0,
        tx_infos: vec![VerifiedTransaction {
            tx_id: "tx".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "old_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: Some(2_000_000),
            fee_microtari: Some(945),
            mined_height: Some(710_357),
            confirmations: None,
            min_confirmations: None,
            tip_height: None,
            confirmed: true,
        }],
        ..Mode1TransferSummary::default()
    };

    summary.backfill_verified_fee_total();

    assert_eq!(summary.fee_microtari, 945);
}

#[test]
fn mode1_summary_uses_confirmed_fee_total() {
    let mut summary = Mode1TransferSummary {
        fee_microtari: 1_000,
        tx_infos: vec![VerifiedTransaction {
            tx_id: "tx".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "old_wallet".to_string(),
            scenario: ScenarioName::S4.as_str().to_string(),
            amount_microtari: Some(1_000_000),
            fee_microtari: Some(945),
            mined_height: Some(710_357),
            confirmations: None,
            min_confirmations: None,
            tip_height: None,
            confirmed: true,
        }],
        ..Mode1TransferSummary::default()
    };

    summary.backfill_verified_fee_total();

    assert_eq!(summary.fee_microtari, 945);
}

#[test]
fn mode1_coin_split_counts_transactions_not_created_outputs() {
    let mut summary = Mode1TransferSummary {
        attempted_batches: 1,
        attempted_payments: 2,
        tx_ids: vec!["tx".to_string()],
        tx_infos: vec![VerifiedTransaction {
            tx_id: "tx".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "old_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: Some(660),
            mined_height: Some(784_029),
            confirmations: Some(3),
            min_confirmations: Some(3),
            tip_height: Some(784_032),
            confirmed: true,
        }],
        ..Mode1TransferSummary::default()
    };

    summary.backfill_verified_fee_total();

    assert_eq!(summary.success_count, 1);
    assert_eq!(summary.failure_count, 0);
    assert!(mode1_s1_complete(&summary, 1));
    assert!(!mode1_s1_complete(&summary, 127));
}

#[test]
fn mode1_s1_observation_uses_constructed_payment_and_change_roles() {
    let mut summary = Mode1TransferSummary {
        tx_ids: vec!["42".to_string()],
        tx_timings: vec![serde_json::json!({
            "tx_id": "42",
            "api_accepted": true
        })],
        tx_infos: vec![VerifiedTransaction {
            tx_id: "42".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "old_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: Some(660),
            mined_height: Some(784_029),
            confirmations: Some(3),
            min_confirmations: Some(3),
            tip_height: Some(784_032),
            confirmed: true,
        }],
        transaction_shapes: BTreeMap::from([(
            "42".to_string(),
            TransactionShape {
                input_count: 1,
                total_output_count: 2,
                output_commitments: vec!["payment".to_string(), "change".to_string()],
            },
        )]),
        ..Mode1TransferSummary::default()
    };
    summary.retain_constructed_output_shape_for_new_tx_ids(
        0,
        ConstructedOutputShape {
            payment_output_count: 1,
            change_output_count: 1,
            total_output_count: 2,
        },
    );

    let metrics = summary.metrics(ScenarioName::S1);

    assert_eq!(
        metrics["constructed_output_shapes_by_tx_id"]["42"],
        serde_json::json!({
            "payment_output_count": 1,
            "change_output_count": 1,
            "total_output_count": 2
        })
    );
    assert_eq!(
        metrics["transaction_observations"][0]["payment_output_count"],
        1
    );
    assert_eq!(
        metrics["transaction_observations"][0]["change_output_count"],
        1
    );
    assert_eq!(
        metrics["transaction_observations"][0]["total_output_count"],
        2
    );
}

#[test]
fn mode1_wallet_rejection_is_not_reported_as_timeout() {
    let summary = Mode1TransferSummary {
        attempted_batches: 1,
        attempted_payments: 1,
        failure_count: 1,
        errors: vec!["Funds are still pending".to_string()],
        tx_timings: vec![serde_json::json!({
            "batch_index": 1,
            "tx_id": null,
            "api_accepted": false,
            "api_error": "Funds are still pending",
            "failure_class": "wallet",
            "error": "Funds are still pending"
        })],
        ..Mode1TransferSummary::default()
    };

    let observations = summary.transaction_observations(ScenarioName::S5);

    assert_eq!(observations[0]["terminal_outcome"], "rejected");
    assert_eq!(observations[0]["failure_class"], "wallet");
    assert_eq!(observations[0]["api_error"], "Funds are still pending");
    assert_eq!(outcome_counts(&observations, 0).rejected, 1);
}

#[test]
fn terminal_ok_status_matches_bounty_status_set() {
    assert!(terminal_ok_status(6));
    for status in [1, 2, 7, 9, 11, 13, 14] {
        assert!(!terminal_ok_status(status));
    }
}

#[test]
fn mode1_s1_does_not_complete_on_mined_unconfirmed_status() {
    let pending = Mode1TransferSummary {
        attempted_batches: 1,
        attempted_payments: 2,
        success_count: 1,
        tx_ids: vec!["42".to_string()],
        tx_infos: vec![VerifiedTransaction {
            tx_id: "42".to_string(),
            status_value: 2,
            mode: "old_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: Some(2_000_000),
            fee_microtari: Some(945),
            mined_height: Some(710_357),
            confirmations: None,
            min_confirmations: None,
            tip_height: None,
            confirmed: terminal_ok_status(2),
        }],
        ..Mode1TransferSummary::default()
    };
    assert!(!mode1_s1_complete(&pending, 1));

    let confirmed = Mode1TransferSummary {
        tx_infos: vec![VerifiedTransaction {
            tx_id: "42".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "old_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: Some(2_000_000),
            fee_microtari: Some(945),
            mined_height: Some(710_357),
            confirmations: None,
            min_confirmations: None,
            tip_height: None,
            confirmed: terminal_ok_status(TX_MINED_CONFIRMED_STATUS),
        }],
        ..pending
    };
    assert!(mode1_s1_complete(&confirmed, 1));
}

#[test]
fn mode1_summary_keeps_mined_unconfirmed_out_of_chain_rows() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::OldWallet.as_str().to_string(),
        empty_mode_profile(ModeName::OldWallet, None),
    );
    let summary = Mode1TransferSummary {
        attempted_batches: 1,
        attempted_payments: 2,
        success_count: 1,
        tx_ids: vec!["42".to_string()],
        tx_infos: vec![VerifiedTransaction {
            tx_id: "42".to_string(),
            status_value: 2,
            mode: "old_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: Some(2_000_000),
            fee_microtari: Some(945),
            mined_height: Some(710_357),
            confirmations: None,
            min_confirmations: None,
            tip_height: None,
            confirmed: false,
        }],
        ..Mode1TransferSummary::default()
    };

    record_mode1_transfer_summary(
        &mut profile,
        ScenarioName::S1,
        &summary,
        Some(1),
        Vec::new(),
    );

    let cell = &profile
        .modes
        .get(ModeName::OldWallet.as_str())
        .unwrap()
        .scenarios[ScenarioName::S1.as_str()];
    assert_eq!(cell.status, CellStatus::Failed);
    assert!(profile.chain_verification.verified_transactions.is_empty());
}

#[test]
fn mode1_summary_rejects_a_confirmed_but_truncated_s1_plan() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::OldWallet.as_str().to_string(),
        empty_mode_profile(ModeName::OldWallet, None),
    );
    let summary = Mode1TransferSummary {
        attempted_batches: 1,
        attempted_payments: 2,
        success_count: 1,
        tx_ids: vec!["42".to_string()],
        tx_infos: vec![VerifiedTransaction {
            tx_id: "42".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "old_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: Some(660),
            mined_height: Some(784_029),
            confirmations: Some(3),
            min_confirmations: Some(3),
            tip_height: Some(784_032),
            confirmed: true,
        }],
        ..Mode1TransferSummary::default()
    };

    record_mode1_transfer_summary(
        &mut profile,
        ScenarioName::S1,
        &summary,
        Some(127),
        Vec::new(),
    );

    let repetition = &profile.modes["old_wallet"].scenarios["S1"].repetitions[0];
    assert_eq!(repetition.status, CellStatus::Failed);
    assert_eq!(repetition.success_count, 1);
    assert_eq!(repetition.failure_count, 126);
}

#[test]
fn mode1_api_not_found_can_reconcile_to_confirmed_chain_outcome() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::OldWallet.as_str().to_string(),
        empty_mode_profile(ModeName::OldWallet, None),
    );
    let summary = Mode1TransferSummary {
        attempted_batches: 1,
        attempted_payments: 1,
        failure_count: 1,
        tx_ids: vec!["42".to_string()],
        errors: vec!["NotFound: Transaction 42 not found within timeout".to_string()],
        tx_timings: vec![serde_json::json!({
            "tx_id": "42",
            "submission_timing_origin": "console_wallet_grpc_timeout_after_transaction_id_ambiguous",
            "submission_started": true,
            "submission_ambiguous": true,
            "api_accepted": null,
            "failure_class": "request_timeout",
            "api_error": "NotFound: Transaction 42 not found within timeout"
        })],
        tx_infos: vec![VerifiedTransaction {
            tx_id: "42".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "old_wallet".to_string(),
            scenario: "S4".to_string(),
            amount_microtari: Some(1_000_000),
            fee_microtari: Some(660),
            mined_height: Some(100),
            confirmations: Some(3),
            min_confirmations: Some(3),
            tip_height: Some(103),
            confirmed: true,
        }],
        ..Mode1TransferSummary::default()
    };

    record_mode1_transfer_summary(&mut profile, ScenarioName::S4, &summary, None, Vec::new());
    let repetition = &profile.modes["old_wallet"].scenarios["S4"].repetitions[0];
    assert_eq!(repetition.status, CellStatus::Ok);
    assert_eq!(repetition.success_count, 1);
    assert_eq!(repetition.failure_count, 0);
    let metrics = repetition.metrics.as_ref().unwrap();
    assert_eq!(metrics["outcome_counts"]["accepted"], 0);
    assert_eq!(metrics["outcome_counts"]["confirmed"], 1);
    assert_eq!(
        metrics["tx_timings"][0]["api_error"],
        summary.tx_timings[0]["api_error"]
    );
}

#[test]
fn mode1_api_not_found_timeout_is_not_reported_as_rejected_without_chain_proof() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::OldWallet.as_str().to_string(),
        empty_mode_profile(ModeName::OldWallet, None),
    );
    let summary = Mode1TransferSummary {
        attempted_batches: 1,
        attempted_payments: 1,
        failure_count: 1,
        tx_ids: vec!["42".to_string()],
        errors: vec!["NotFound: Transaction 42 not found within timeout".to_string()],
        tx_timings: vec![serde_json::json!({
            "tx_id": "42",
            "submission_timing_origin": "console_wallet_grpc_timeout_after_transaction_id_ambiguous",
            "submission_started": true,
            "submission_ambiguous": true,
            "api_accepted": null,
            "failure_class": "request_timeout",
            "api_error": "NotFound: Transaction 42 not found within timeout"
        })],
        ..Mode1TransferSummary::default()
    };

    record_mode1_transfer_summary(&mut profile, ScenarioName::S4, &summary, None, Vec::new());
    let repetition = &profile.modes["old_wallet"].scenarios["S4"].repetitions[0];
    let metrics = repetition.metrics.as_ref().unwrap();
    let observation = &metrics["transaction_observations"][0];

    assert_eq!(observation["submission_started"], true);
    assert!(observation["api_accepted"].is_null());
    assert_eq!(observation["failure_class"], "request_timeout");
    assert_eq!(observation["terminal_outcome"], "timed_out");
    assert_eq!(metrics["outcome_counts"]["rejected"], 0);
    assert_eq!(metrics["outcome_counts"]["timed_out"], 1);
}

#[test]
fn base_node_endpoint_url_uses_http_surface() {
    assert_eq!(
        base_node_endpoint_url("https://rpc.esmeralda.tari.com", "/get_tip_info")
            .unwrap()
            .as_str(),
        "https://rpc.esmeralda.tari.com/get_tip_info"
    );
}

#[test]
fn mode2_transaction_query_url_uses_kernel_signature_params() {
    let query = Mode2KernelQuery {
        excess_sig_nonce: vec![0xab, 0xcd],
        excess_sig: vec![0x12, 0x34],
        fee_microtari: Some(42),
    };
    let url = mode2_transaction_query_url("https://rpc.esmeralda.tari.com", &query).unwrap();

    assert_eq!(
        url.as_str(),
        "https://rpc.esmeralda.tari.com/transactions?excess_sig_nonce=abcd&excess_sig_sig=1234"
    );
}

#[test]
fn mode2_transaction_query_status_requires_depth() {
    let mined = TxQueryResponse {
        location: TxLocation::Mined,
        mined_height: Some(100),
        mined_header_hash: None,
        mined_timestamp: None,
    };
    assert_eq!(
        mode2_transaction_query_status(&mined, Some(102), 3),
        (2, false)
    );
    assert_eq!(
        mode2_transaction_query_status(&mined, Some(103), 3),
        (TX_MINED_CONFIRMED_STATUS, true)
    );

    let mempool = TxQueryResponse {
        location: TxLocation::InMempool,
        mined_height: None,
        mined_header_hash: None,
        mined_timestamp: None,
    };
    assert_eq!(
        mode2_transaction_query_status(&mempool, Some(103), 3),
        (1, false)
    );

    let not_stored = TxQueryResponse {
        location: TxLocation::NotStored,
        mined_height: None,
        mined_header_hash: None,
        mined_timestamp: None,
    };
    assert_eq!(
        mode2_transaction_query_status(&not_stored, Some(103), 3),
        (0, false)
    );

    let none = TxQueryResponse {
        location: TxLocation::None,
        mined_height: None,
        mined_header_hash: None,
        mined_timestamp: None,
    };
    assert_eq!(
        mode2_transaction_query_status(&none, Some(103), 3),
        (0, false)
    );
}

#[test]
fn mode2_kernel_query_rejects_invalid_serialized_transaction() {
    let error = mode2_kernel_query_from_serialized_transaction(b"not-json")
        .expect_err("invalid transaction must fail");
    assert!(format!("{error:#}").contains("deserializing Mode 2 transaction"));
}

#[test]
fn mode2_verification_confirmed_requires_every_tx_confirmed() {
    let tx_ids = vec!["1".to_string(), "2".to_string()];
    let one_confirmed = Mode2VerificationResult {
        observed_transactions: vec![VerifiedTransaction {
            tx_id: "1".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "new_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: Some(10),
            mined_height: Some(100),
            confirmations: None,
            min_confirmations: None,
            tip_height: None,
            confirmed: true,
        }],
        observations: Vec::new(),
        used_base_node_query: true,
        confirmation_observed_offsets_ms: BTreeMap::new(),
        transaction_shapes: BTreeMap::new(),
    };
    assert!(!mode2_verification_confirmed(&one_confirmed, &tx_ids));

    let all_confirmed = Mode2VerificationResult {
        observed_transactions: vec![
            one_confirmed.observed_transactions[0].clone(),
            VerifiedTransaction {
                tx_id: "2".to_string(),
                status_value: TX_MINED_CONFIRMED_STATUS,
                mode: "new_wallet".to_string(),
                scenario: ScenarioName::S1.as_str().to_string(),
                amount_microtari: None,
                fee_microtari: Some(10),
                mined_height: Some(101),
                confirmations: None,
                min_confirmations: None,
                tip_height: None,
                confirmed: true,
            },
        ],
        observations: Vec::new(),
        used_base_node_query: true,
        confirmation_observed_offsets_ms: BTreeMap::new(),
        transaction_shapes: BTreeMap::new(),
    };
    assert!(mode2_verification_confirmed(&all_confirmed, &tx_ids));
}

#[tokio::test]
async fn mode2_verification_returns_immediately_without_tx_ids() {
    let config = Config::default();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("unused-wallet.db");

    let (verification, attempts, wall_ms) =
        verify_mode2_transactions_until_confirmed(&config, &db_path, &[], ScenarioName::S4)
            .await
            .unwrap();

    assert_eq!(attempts, 0);
    assert_eq!(wall_ms, 0);
    assert!(verification.observed_transactions.is_empty());
    assert!(verification.observations.is_empty());
    assert!(!verification.used_base_node_query);
}

#[test]
fn scan_checkpoint_gates_missing_prerequisites() {
    assert!(!ScanCheckpoint::PostS1Blocked.runnable());
    assert!(ScanCheckpoint::PostS1Partial.runnable());
    assert!(
        ScanCheckpoint::PostS1Blocked
            .blocked_note(ScenarioName::S2)
            .contains("post_s1_blocked")
    );
}

#[test]
fn b0_reuses_the_benchmark_seed_with_encoded_birthday_zero() {
    let seed = CipherSeed::random();
    let words = crate::seeds::seed_words_from_seed(&seed).unwrap();
    let spec = FreshScanSpec {
        scenario: ScenarioName::B0,
        wallet_state: FreshScanWalletState::EmptyGenesis,
        checkpoint: ScanCheckpoint::Empty,
    };
    let b0_seed = spec.seed(Some(&words)).unwrap();
    let original =
        crate::seeds::material_from_seed(WalletRole::NewWallet, "TEST_ORIGINAL".to_string(), seed)
            .unwrap();
    let rewritten =
        crate::seeds::material_from_seed(WalletRole::NewWallet, "TEST_B0".to_string(), b0_seed)
            .unwrap();

    assert_eq!(rewritten.birthday, 0);
    assert_eq!(rewritten.address, original.address);
}

#[test]
fn blocked_checkpoint_scan_records_failed_repetition() {
    let mut cell = ScenarioCell {
        scenario: ScenarioName::S6,
        surface: "minotari_library".to_string(),
        status: CellStatus::ReadyForLiveRun,
        repetitions: Vec::new(),
        median_wall_ms: None,
        spread_wall_ms: None,
        all_runs_median_wall_ms: None,
        notes: Vec::new(),
    };
    let spec = FreshScanSpec {
        scenario: ScenarioName::S6,
        wallet_state: FreshScanWalletState::FundedGenesis,
        checkpoint: ScanCheckpoint::PostS1Blocked,
    };

    record_blocked_checkpoint_scan(&mut cell, spec);

    assert_eq!(cell.status, CellStatus::BlockedPrerequisite);
    assert_eq!(cell.median_wall_ms, None);
    assert_eq!(cell.spread_wall_ms, None);
    assert_eq!(cell.repetitions.len(), 1);
    assert_eq!(cell.repetitions[0].status, CellStatus::BlockedPrerequisite);
    assert_eq!(cell.repetitions[0].wall_ms, None);
    assert_eq!(cell.repetitions[0].success_count, 0);
    assert_eq!(cell.repetitions[0].failure_count, 0);
    assert_eq!(cell.repetitions[0].fee_microtari, None);
    assert_eq!(cell.repetitions[0].error, None);
    let metrics = cell.repetitions[0].metrics.as_ref().unwrap();
    assert_eq!(metrics["blocked_prerequisite"], serde_json::json!(true));
    assert_eq!(
        metrics["scan_checkpoint"],
        serde_json::json!("post_s1_blocked")
    );
}

#[test]
fn blocked_prerequisite_records_unmeasured_repetition() {
    let mut cell = ScenarioCell {
        scenario: ScenarioName::S5,
        surface: "minotari_library".to_string(),
        status: CellStatus::ReadyForLiveRun,
        repetitions: Vec::new(),
        median_wall_ms: None,
        spread_wall_ms: None,
        all_runs_median_wall_ms: None,
        notes: Vec::new(),
    };

    record_blocked_prerequisite_cell(&mut cell, ScenarioName::S5, "S4");

    assert_eq!(cell.status, CellStatus::BlockedPrerequisite);
    assert_eq!(cell.repetitions.len(), 1);
    assert_eq!(cell.repetitions[0].status, CellStatus::BlockedPrerequisite);
    assert_eq!(cell.repetitions[0].success_count, 0);
    assert_eq!(cell.repetitions[0].failure_count, 0);
    assert_eq!(cell.repetitions[0].wall_ms, None);
    assert_eq!(cell.repetitions[0].fee_microtari, None);
    assert_eq!(cell.repetitions[0].error, None);
    let metrics = cell.repetitions[0].metrics.as_ref().unwrap();
    assert_eq!(metrics["blocked_prerequisite"], serde_json::json!(true));
    assert_eq!(metrics["prerequisite"], serde_json::json!("S4"));
}

#[test]
fn mode1_scan_grpc_address_offsets_port() {
    let spec = FreshScanSpec {
        scenario: ScenarioName::S3,
        wallet_state: FreshScanWalletState::FundedBirthday { birthday: 123 },
        checkpoint: ScanCheckpoint::PostS1,
    };
    assert_eq!(
        mode1_scan_grpc_address("http://127.0.0.1:18143", spec, 2).unwrap(),
        "http://127.0.0.1:18445"
    );
}

#[test]
fn mode1_scan_grpc_addresses_cover_every_fresh_scan_port() {
    let mut config = Config::default();
    config.benchmark.scan_repetitions = 2;

    assert_eq!(
        mode1_scan_grpc_addresses(&config).unwrap(),
        [
            18244, 18245, 18344, 18345, 18444, 18445, 18544, 18545, 18644, 18645,
        ]
        .map(|port| format!("http://127.0.0.1:{port}"))
    );
}

#[test]
fn recipient_batches_preserve_order_without_chunks() {
    let recipients = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    assert_eq!(
        recipient_batches(recipients, 2),
        vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string()]
        ]
    );
}

#[test]
fn pp_scan_cells_are_not_applicable_when_companion_scans_are_disabled() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::OldWallet.as_str().to_string(),
        empty_mode_profile(ModeName::OldWallet, None),
    );
    profile.modes.insert(
        ModeName::NewWallet.as_str().to_string(),
        empty_mode_profile(ModeName::NewWallet, None),
    );
    profile.modes.insert(
        ModeName::PaymentProcessor.as_str().to_string(),
        empty_mode_profile(ModeName::PaymentProcessor, None),
    );

    annotate_fresh_scan_cells_disabled(&mut profile);

    let pp_b0 = &profile
        .modes
        .get(ModeName::PaymentProcessor.as_str())
        .unwrap()
        .scenarios[ScenarioName::B0.as_str()];
    assert_eq!(pp_b0.status, CellStatus::NotApplicable);
    assert!(
        pp_b0
            .notes
            .iter()
            .any(|note| note.contains("PP has no direct scan API"))
    );

    let new_b0 = &profile
        .modes
        .get(ModeName::NewWallet.as_str())
        .unwrap()
        .scenarios[ScenarioName::B0.as_str()];
    assert_eq!(new_b0.status, CellStatus::NotApplicable);
    let old_b0 = &profile
        .modes
        .get(ModeName::OldWallet.as_str())
        .unwrap()
        .scenarios[ScenarioName::B0.as_str()];
    assert_eq!(old_b0.status, CellStatus::NotApplicable);
    assert!(
        old_b0
            .notes
            .iter()
            .any(|note| note.contains("fresh live scan cell disabled"))
    );
}

#[test]
fn operational_refresh_requires_the_fixed_target_cursor_and_hash() {
    let mut report = ScanToTipReport {
        wall_ms: 10,
        target_tip: 100,
        target_hash: "target".to_string(),
        completion_tip: 105,
        completion_hash: "completion".to_string(),
        max_height: 99,
        cursor_hash: Some("partial".to_string()),
        scan_invocations: 1,
        no_progress_attempts: 0,
        stopped_without_progress: false,
        last_more_blocks: Some(true),
    };

    let error = require_complete_operational_refresh(&report).unwrap_err();
    assert!(error.to_string().contains("persisted cursor 99"));

    report.max_height = 100;
    let error = require_complete_operational_refresh(&report).unwrap_err();
    assert!(error.to_string().contains("cursor hash mismatch"));

    report.cursor_hash = Some("target".to_string());
    require_complete_operational_refresh(&report).unwrap();
}

#[test]
fn scan_report_stop_reason_uses_the_wallet_cursor() {
    let report = ScanToTipReport {
        wall_ms: 10,
        target_tip: 100,
        target_hash: "target".to_string(),
        completion_tip: 105,
        completion_hash: "completion".to_string(),
        max_height: 99,
        cursor_hash: Some("partial".to_string()),
        scan_invocations: 1,
        no_progress_attempts: 0,
        stopped_without_progress: false,
        last_more_blocks: Some(true),
    };
    let mut metrics = serde_json::Map::new();

    report.insert_metrics(&mut metrics);

    assert_eq!(
        metrics["scan_stop_reason"],
        serde_json::json!("scanner_returned_below_fixed_target")
    );
}

#[test]
fn fixed_tip_scan_continues_only_while_the_scanner_advances_with_work_remaining() {
    assert!(should_continue_fixed_tip_scan(90, 95, 100, true));
    assert!(!should_continue_fixed_tip_scan(95, 95, 100, true));
    assert!(!should_continue_fixed_tip_scan(95, 99, 100, false));
    assert!(!should_continue_fixed_tip_scan(95, 100, 100, true));
}

#[test]
fn timed_out_scan_metrics_preserve_measured_evidence() {
    let measurement = ScanMeasurement {
        birthday: 0,
        birthday_height: 0,
        birthday_offset_days: 0,
        birthday_start_height: 0,
        max_height: 42,
        target_cursor_hash: None,
        cursor_hash: Some("partial".to_string()),
        wall_ms: 1_234,
        available_microtari: 7,
        tip_start: Some(100),
        tip_end: Some(100),
        tip_target_hash: Some("target".to_string()),
        tip_completion: Some(101),
        tip_completion_hash: Some("completion".to_string()),
        detected_outputs: 2,
        history_transactions: 1,
        history_tx_ids: BTreeSet::new(),
        recovered_output_commitments: BTreeSet::new(),
        spendable_outputs: 1,
        resource_peaks: ResourcePeaks {
            peak_rss_bytes: Some(1_024),
            peak_cpu_percent: Some(12.5),
        },
        expectations: ScanExpectations::default(),
        tip_lag_tolerance_blocks: 0,
        scan_invocations: 1,
        scan_no_progress_attempts: 0,
        scan_stopped_without_progress: false,
        scan_last_more_blocks: None,
        scan_timeout_ms: Some(1_000),
        scan_error: Some("wallet scan exceeded its fixed deadline".to_string()),
        allow_target_overshoot: false,
    };
    let metrics = measurement.metrics(
        "new_wallet",
        FreshScanSpec {
            scenario: ScenarioName::S2,
            wallet_state: FreshScanWalletState::FundedGenesis,
            checkpoint: ScanCheckpoint::PostS1,
        },
    );

    assert!(!measurement.scan_verification_ok());
    assert_eq!(metrics["scan_timed_out"], serde_json::json!(true));
    assert_eq!(metrics["scan_timeout_ms"], serde_json::json!(1_000));
    assert_eq!(metrics["H_tip_end"], serde_json::json!(100));
    assert_eq!(metrics["max_height"], serde_json::json!(42));
    assert_eq!(metrics["scan_invocations"], serde_json::json!(1));
    assert_eq!(metrics["peak_rss_bytes"], serde_json::json!(1_024));
    assert_eq!(metrics["peak_cpu_percent"], serde_json::json!(12.5));
    assert_eq!(
        metrics["scan_stop_reason"],
        serde_json::json!("scan_timeout")
    );
}

#[test]
fn birthday_scan_metrics_include_blocks_per_second() {
    let measurement = ScanMeasurement {
        birthday: 1_635,
        birthday_height: 705_000,
        birthday_offset_days: 2,
        birthday_start_height: 700_000,
        max_height: 711_305,
        target_cursor_hash: Some("target".to_string()),
        cursor_hash: Some("target".to_string()),
        wall_ms: 10_000,
        available_microtari: 1,
        tip_start: Some(711_300),
        tip_end: Some(711_305),
        tip_target_hash: Some("target".to_string()),
        tip_completion: Some(711_306),
        tip_completion_hash: Some("completion".to_string()),
        detected_outputs: 1,
        history_transactions: 1,
        history_tx_ids: BTreeSet::new(),
        recovered_output_commitments: BTreeSet::new(),
        spendable_outputs: 1,
        resource_peaks: ResourcePeaks::default(),
        expectations: ScanExpectations::default(),
        tip_lag_tolerance_blocks: 3,
        scan_invocations: 1,
        scan_no_progress_attempts: 0,
        scan_stopped_without_progress: false,
        scan_last_more_blocks: None,
        scan_timeout_ms: None,
        scan_error: None,
        allow_target_overshoot: false,
    };
    let spec = FreshScanSpec {
        scenario: ScenarioName::S3,
        wallet_state: FreshScanWalletState::FundedBirthday { birthday: 1_635 },
        checkpoint: ScanCheckpoint::PostS1,
    };
    let metrics = measurement.metrics("new_wallet", spec);

    assert_eq!(metrics["H_birth"], serde_json::json!(705_000));
    assert_eq!(metrics["blocks_scanned"], serde_json::json!(6_305));
    assert_eq!(
        metrics["effective_blocks_scanned"],
        serde_json::json!(11_305)
    );
    assert!(metrics["blocks_per_sec"].as_f64().unwrap() > 0.0);
    assert!(metrics["effective_blocks_per_sec"].as_f64().unwrap() > 0.0);
    assert!(measurement.scan_reached_tip());
}

#[test]
fn scan_verification_fails_when_scan_stops_far_below_tip() {
    let measurement = ScanMeasurement {
        birthday: 0,
        birthday_height: 0,
        birthday_offset_days: 0,
        birthday_start_height: 0,
        max_height: 627_100,
        target_cursor_hash: None,
        cursor_hash: Some("partial".to_string()),
        wall_ms: 10_000,
        available_microtari: 0,
        tip_start: Some(726_900),
        tip_end: Some(726_905),
        tip_target_hash: Some("target".to_string()),
        tip_completion: Some(726_906),
        tip_completion_hash: Some("completion".to_string()),
        detected_outputs: 0,
        history_transactions: 0,
        history_tx_ids: BTreeSet::new(),
        recovered_output_commitments: BTreeSet::new(),
        spendable_outputs: 0,
        resource_peaks: ResourcePeaks::default(),
        expectations: ScanExpectations {
            expected_outputs: Some(0),
            expected_available_microtari: Some(0),
            expected_history_transactions: Some(0),
            ..ScanExpectations::default()
        },
        tip_lag_tolerance_blocks: 3,
        scan_invocations: 1,
        scan_no_progress_attempts: 2,
        scan_stopped_without_progress: true,
        scan_last_more_blocks: Some(true),
        scan_timeout_ms: None,
        scan_error: None,
        allow_target_overshoot: false,
    };

    assert!(!measurement.scan_verification_ok());
    assert_eq!(measurement.tip_lag_blocks(), Some(99_805));
    assert!(
        measurement
            .scan_verification_error()
            .contains("max_height=627100")
    );
}

#[test]
fn library_scan_verifies_chain_history_by_output_commitment() {
    let expected = BTreeSet::from(["commitment-a".to_string()]);
    let measurement = ScanMeasurement {
        birthday: 0,
        birthday_height: 0,
        birthday_offset_days: 0,
        birthday_start_height: 0,
        max_height: 10,
        target_cursor_hash: Some("target".to_string()),
        cursor_hash: Some("target".to_string()),
        wall_ms: 1,
        available_microtari: 1,
        tip_start: Some(10),
        tip_end: Some(10),
        tip_target_hash: Some("target".to_string()),
        tip_completion: Some(10),
        tip_completion_hash: Some("target".to_string()),
        detected_outputs: 1,
        history_transactions: 1,
        history_tx_ids: BTreeSet::from(["wallet-local-id".to_string()]),
        recovered_output_commitments: expected.clone(),
        spendable_outputs: 1,
        resource_peaks: ResourcePeaks::default(),
        expectations: ScanExpectations {
            expected_outputs: Some(1),
            expected_available_microtari: Some(1),
            expected_output_commitments: BTreeMap::from([("chain-tx-id".to_string(), expected)]),
            ..ScanExpectations::default()
        },
        tip_lag_tolerance_blocks: 0,
        scan_invocations: 1,
        scan_no_progress_attempts: 0,
        scan_stopped_without_progress: false,
        scan_last_more_blocks: Some(false),
        scan_timeout_ms: None,
        scan_error: None,
        allow_target_overshoot: false,
    };

    assert!(measurement.scan_verification_ok());
    let metrics = measurement.metrics(
        "new_wallet",
        FreshScanSpec {
            scenario: ScenarioName::S2,
            wallet_state: FreshScanWalletState::FundedGenesis,
            checkpoint: ScanCheckpoint::PostS1,
        },
    );
    assert_eq!(
        metrics["missing_history_output_tx_ids"],
        serde_json::json!([])
    );
    assert_eq!(metrics["history_identities_match_expected"], true);
}

#[test]
fn scan_verification_requires_exact_tip_for_scan_scenarios() {
    let measurement = ScanMeasurement {
        birthday: 0,
        birthday_height: 0,
        birthday_offset_days: 0,
        birthday_start_height: 0,
        max_height: 726_902,
        target_cursor_hash: None,
        cursor_hash: Some("partial".to_string()),
        wall_ms: 10_000,
        available_microtari: 0,
        tip_start: Some(726_900),
        tip_end: Some(726_905),
        tip_target_hash: Some("target".to_string()),
        tip_completion: Some(726_906),
        tip_completion_hash: Some("completion".to_string()),
        detected_outputs: 0,
        history_transactions: 0,
        history_tx_ids: BTreeSet::new(),
        recovered_output_commitments: BTreeSet::new(),
        spendable_outputs: 0,
        resource_peaks: ResourcePeaks::default(),
        expectations: ScanExpectations {
            expected_outputs: Some(0),
            expected_available_microtari: Some(0),
            expected_history_transactions: Some(0),
            ..ScanExpectations::default()
        },
        tip_lag_tolerance_blocks: 0,
        scan_invocations: 1,
        scan_no_progress_attempts: 0,
        scan_stopped_without_progress: false,
        scan_last_more_blocks: None,
        scan_timeout_ms: None,
        scan_error: None,
        allow_target_overshoot: false,
    };

    assert!(!measurement.scan_verification_ok());
    assert_eq!(measurement.tip_lag_blocks(), Some(3));
    let metrics = measurement.metrics(
        "new_wallet",
        FreshScanSpec {
            scenario: ScenarioName::B0,
            wallet_state: FreshScanWalletState::EmptyGenesis,
            checkpoint: ScanCheckpoint::Empty,
        },
    );
    assert_eq!(metrics["scan_reached_tip"], serde_json::json!(false));
    assert_eq!(metrics["tip_lag_blocks"], serde_json::json!(3));
    assert_eq!(
        metrics["H_scan_target_cursor_hash"],
        serde_json::Value::Null
    );
    assert_eq!(metrics["H_scan_cursor_hash"], serde_json::json!("partial"));
    assert_eq!(
        metrics["scan_stop_reason"],
        serde_json::json!("scanner_returned_below_fixed_target")
    );
}

#[test]
fn scan_verification_rejects_unapproved_overshoot_and_target_hash_mismatch() {
    let mut measurement = ScanMeasurement {
        birthday: 0,
        birthday_height: 0,
        birthday_offset_days: 0,
        birthday_start_height: 0,
        max_height: 101,
        target_cursor_hash: Some("target".to_string()),
        cursor_hash: Some("target".to_string()),
        wall_ms: 1,
        available_microtari: 0,
        tip_start: Some(100),
        tip_end: Some(100),
        tip_target_hash: Some("target".to_string()),
        tip_completion: Some(101),
        tip_completion_hash: Some("completion".to_string()),
        detected_outputs: 0,
        history_transactions: 0,
        history_tx_ids: BTreeSet::new(),
        recovered_output_commitments: BTreeSet::new(),
        spendable_outputs: 0,
        resource_peaks: ResourcePeaks::default(),
        expectations: ScanExpectations::default(),
        tip_lag_tolerance_blocks: 0,
        scan_invocations: 1,
        scan_no_progress_attempts: 0,
        scan_stopped_without_progress: false,
        scan_last_more_blocks: None,
        scan_timeout_ms: None,
        scan_error: None,
        allow_target_overshoot: false,
    };

    assert!(!measurement.scan_verification_ok());
    measurement.max_height = 100;
    measurement.target_cursor_hash = Some("other".to_string());
    assert!(!measurement.scan_verification_ok());
    measurement.target_cursor_hash = Some("target".to_string());
    assert!(measurement.scan_verification_ok());
}

#[test]
fn mode1_scan_verification_accepts_honest_overshoot_at_immutable_target() {
    let mut measurement = ScanMeasurement {
        birthday: 0,
        birthday_height: 0,
        birthday_offset_days: 0,
        birthday_start_height: 0,
        max_height: 101,
        target_cursor_hash: Some("target".to_string()),
        cursor_hash: Some("completion".to_string()),
        wall_ms: 1,
        available_microtari: 0,
        tip_start: Some(100),
        tip_end: Some(100),
        tip_target_hash: Some("target".to_string()),
        tip_completion: Some(101),
        tip_completion_hash: Some("completion".to_string()),
        detected_outputs: 0,
        history_transactions: 0,
        history_tx_ids: BTreeSet::new(),
        recovered_output_commitments: BTreeSet::new(),
        spendable_outputs: 0,
        resource_peaks: ResourcePeaks::default(),
        expectations: ScanExpectations::default(),
        tip_lag_tolerance_blocks: 0,
        scan_invocations: 1,
        scan_no_progress_attempts: 0,
        scan_stopped_without_progress: false,
        scan_last_more_blocks: None,
        scan_timeout_ms: None,
        scan_error: None,
        allow_target_overshoot: true,
    };

    assert!(measurement.scan_verification_ok());
    let metrics = measurement.metrics(
        "old_wallet",
        FreshScanSpec {
            scenario: ScenarioName::S2,
            wallet_state: FreshScanWalletState::FundedGenesis,
            checkpoint: ScanCheckpoint::PostS1,
        },
    );
    assert_eq!(metrics["H_tip_end"], serde_json::json!(100));
    assert_eq!(metrics["max_height"], serde_json::json!(101));
    assert_eq!(metrics["H_scan_target_cursor_hash"], "target");
    assert_eq!(metrics["H_scan_cursor_hash"], "completion");
    assert_eq!(metrics["scan_overshoot_blocks"], serde_json::json!(1));
    assert_eq!(metrics["scan_reached_tip"], true);

    measurement.target_cursor_hash = Some("wrong".to_string());
    assert!(!measurement.scan_verification_ok());
}

#[test]
fn pp_s4_observation_uses_s4_budget() {
    let mut config = Config::default();
    config.benchmark.s4_t_budget_secs = 17;
    config.timeouts.confirmation_secs = 999;

    assert_eq!(
        pp_observation_timeout(&config, ScenarioName::S4),
        Duration::from_secs(17)
    );
    assert_eq!(
        pp_observation_timeout(&config, ScenarioName::S1),
        Duration::from_secs(999)
    );
}

#[test]
fn pp_retry_count_is_not_terminal_progress() {
    let snapshot = PaymentProcessorDbSnapshot {
        batches: vec![payment_processor::PaymentBatchSnapshot {
            id: "batch".to_string(),
            status: "PENDING_BATCHING".to_string(),
            retry_count: 3,
            error_message: None,
            has_unsigned_tx: false,
            has_signed_tx: false,
            mined_height: None,
            chain_tx_id: None,
            fee_microtari: None,
            kernel_excess_sig_nonce: None,
            kernel_excess_sig: None,
            input_count: None,
            total_output_count: None,
            output_commitments: None,
        }],
        payments: Vec::new(),
    };
    assert!(!pp_snapshot_has_progress_or_error(&snapshot));
}

#[test]
fn pp_terminal_wait_requires_c_min_proof_for_confirmed_batches() {
    let batch_ids = vec!["batch".to_string()];
    let awaiting = PaymentProcessorDbSnapshot {
        batches: vec![payment_processor::PaymentBatchSnapshot {
            id: "batch".to_string(),
            status: "AWAITING_CONFIRMATION".to_string(),
            retry_count: 0,
            error_message: None,
            has_unsigned_tx: true,
            has_signed_tx: true,
            mined_height: None,
            chain_tx_id: None,
            fee_microtari: None,
            kernel_excess_sig_nonce: None,
            kernel_excess_sig: None,
            input_count: None,
            total_output_count: None,
            output_commitments: None,
        }],
        payments: Vec::new(),
    };
    assert!(!pp_observation_complete(
        &batch_ids,
        &awaiting,
        &BTreeMap::new()
    ));

    let confirmed = PaymentProcessorDbSnapshot {
        batches: vec![payment_processor::PaymentBatchSnapshot {
            id: "batch".to_string(),
            status: "CONFIRMED".to_string(),
            retry_count: 0,
            error_message: None,
            has_unsigned_tx: true,
            has_signed_tx: true,
            mined_height: Some(42),
            chain_tx_id: None,
            fee_microtari: None,
            kernel_excess_sig_nonce: None,
            kernel_excess_sig: None,
            input_count: None,
            total_output_count: None,
            output_commitments: None,
        }],
        payments: Vec::new(),
    };
    assert!(!pp_observation_complete(
        &batch_ids,
        &confirmed,
        &BTreeMap::new()
    ));
    let proofs = BTreeMap::from([(
        "batch".to_string(),
        PpChainProof {
            chain_tx_id: "42".to_string(),
            amount_microtari: 100,
            fee_microtari: 660,
            mined_height: 42,
            tip_height: 45,
            confirmations: 3,
            min_confirmations: 3,
            shape: TransactionShape {
                input_count: 1,
                total_output_count: 2,
                output_commitments: Vec::new(),
            },
        },
    )]);
    assert!(pp_observation_complete(&batch_ids, &confirmed, &proofs));
}

#[test]
fn mode1_coin_split_timing_preserves_accepted_transaction_identity() {
    let outcome = Mode1TransferOutcome {
        success_count: 1,
        failure_count: 0,
        fee_microtari: 0,
        tx_ids: vec!["42".to_string()],
        errors: Vec::new(),
        tx_timings: vec![serde_json::json!({
            "tx_id": "42",
            "api_accepted": true
        })],
    }
    .with_rpc_timing(1, 25);

    assert_eq!(outcome.failure_count, 0);
    assert!(outcome.errors.is_empty());
    assert_eq!(outcome.tx_timings[0]["tx_id"], "42");
    assert_eq!(outcome.tx_timings[0]["api_accepted"], true);
    assert_eq!(outcome.tx_timings[0]["grpc_round_trip_ms"], 25);
    assert!(outcome.tx_timings[0]["construction_complete_ms"].is_null());
}

#[test]
fn mode1_arm_deadline_is_ambiguous_and_never_classified_as_rejected() {
    let mut summary = Mode1TransferSummary::default();
    summary.record_deadline_timeout(
        3,
        1,
        17,
        900_000,
        vec!["recipient".to_string()],
        "absolute deadline expired".to_string(),
    );

    let observation = &summary.transaction_observations(ScenarioName::S4)[0];
    assert_eq!(observation["terminal_outcome"], "timed_out");
    assert_eq!(observation["failure_class"], "arm_deadline");
    assert!(observation["api_accepted"].is_null());
    assert!(observation["construction_ms"].is_null());
    assert!(observation["construction_complete_offset_ms"].is_null());
}

#[test]
fn mode1_grpc_transport_failure_is_ambiguous_not_rejected() {
    let mut ambiguous = Mode1TransferSummary::default();
    ambiguous.record_batch(
        1,
        1,
        5,
        25,
        vec!["recipient".to_string()],
        Err(tonic::Status::unavailable("connection lost").into()),
    );
    let observation = &ambiguous.transaction_observations(ScenarioName::S4)[0];
    assert_eq!(observation["terminal_outcome"], "stalled");
    assert_eq!(observation["submission_started"], true);
    assert_eq!(observation["failure_class"], "transport");
    assert!(observation["api_accepted"].is_null());

    let mut rejected = Mode1TransferSummary::default();
    rejected.record_batch(
        1,
        1,
        5,
        25,
        vec!["recipient".to_string()],
        Err(tonic::Status::invalid_argument("bad address").into()),
    );
    let observation = &rejected.transaction_observations(ScenarioName::S4)[0];
    assert_eq!(observation["terminal_outcome"], "rejected");
    assert_eq!(observation["submission_started"], true);
    assert_eq!(observation["failure_class"], "wallet");
    assert_eq!(observation["api_accepted"], false);
}

#[test]
fn pp_terminal_wait_does_not_stop_while_an_accepted_batch_is_pending() {
    let snapshot = PaymentProcessorDbSnapshot {
        batches: vec![payment_processor::PaymentBatchSnapshot {
            id: "batch".to_string(),
            status: "PENDING_BATCHING".to_string(),
            retry_count: 1,
            error_message: Some("signing failed".to_string()),
            has_unsigned_tx: false,
            has_signed_tx: false,
            mined_height: None,
            chain_tx_id: None,
            fee_microtari: None,
            kernel_excess_sig_nonce: None,
            kernel_excess_sig: None,
            input_count: None,
            total_output_count: None,
            output_commitments: None,
        }],
        payments: Vec::new(),
    };
    assert!(!pp_observation_complete(
        &["batch".to_string()],
        &snapshot,
        &BTreeMap::new()
    ));
}

#[test]
fn pp_pending_outcomes_keep_uuid_fee_and_deadline_taxonomy() {
    let timing = serde_json::json!({
        "batch_index": 1,
        "batch_id": "2f1c928d-9516-48c8-b926-e5cbe33b3168",
        "api_accepted": true,
        "payment_count": 1,
        "recipients": ["recipient"]
    });
    let snapshot = PaymentProcessorDbSnapshot {
        batches: vec![payment_processor::PaymentBatchSnapshot {
            id: "2f1c928d-9516-48c8-b926-e5cbe33b3168".to_string(),
            status: "AWAITING_CONFIRMATION".to_string(),
            retry_count: 0,
            error_message: None,
            has_unsigned_tx: true,
            has_signed_tx: true,
            mined_height: None,
            chain_tx_id: None,
            fee_microtari: Some(700),
            kernel_excess_sig_nonce: None,
            kernel_excess_sig: None,
            input_count: None,
            total_output_count: None,
            output_commitments: None,
        }],
        payments: Vec::new(),
    };
    let mut summary = PpScenarioSummary {
        tx_timings: vec![timing],
        db_snapshot: Some(snapshot),
        ..PpScenarioSummary::default()
    };

    let stalled = summary.transaction_observations();
    assert_eq!(stalled[0]["terminal_outcome"], "stalled");
    assert_eq!(stalled[0]["submission_started"], true);
    assert_eq!(stalled[0]["fee_disposition"], "proposed_unresolved");
    assert_eq!(stalled[0]["fee_microtari"], 700);
    assert_eq!(
        stalled[0]["batch_id"],
        "2f1c928d-9516-48c8-b926-e5cbe33b3168"
    );

    summary.extra_metrics.insert(
        "db_observation_timed_out".to_string(),
        serde_json::json!(true),
    );
    let timed_out = summary.transaction_observations();
    assert_eq!(timed_out[0]["terminal_outcome"], "timed_out");
    assert_eq!(timed_out[0]["failure_class"], "arm_deadline");
}

#[test]
fn pp_arm_deadline_is_ambiguous_and_never_classified_as_rejected() {
    let mut summary = PpScenarioSummary::default();
    summary.record_deadline_timeout(
        7,
        21,
        900_000,
        vec!["recipient".to_string()],
        "absolute deadline expired".to_string(),
    );

    let observation = &summary.transaction_observations()[0];
    assert_eq!(observation["terminal_outcome"], "timed_out");
    assert_eq!(observation["failure_class"], "arm_deadline");
    assert!(observation["api_accepted"].is_null());
    assert!(observation["construction_ms"].is_null());
    assert!(observation["construction_complete_offset_ms"].is_null());
}

#[test]
fn pp_construction_timing_requires_first_persisted_unsigned_transaction_observation() {
    let mut summary = PpScenarioSummary {
        tx_timings: vec![serde_json::json!({
            "batch_index": 1,
            "batch_id": "batch",
            "submit_offset_ms": 25,
            "construction_complete_offset_ms": null,
            "construction_ms": null,
            "construction_timing_origin": null,
            "api_accepted": true
        })],
        ..PpScenarioSummary::default()
    };
    let mut snapshot = PaymentProcessorDbSnapshot {
        batches: vec![payment_processor::PaymentBatchSnapshot {
            id: "batch".to_string(),
            status: "PENDING_BATCHING".to_string(),
            retry_count: 0,
            error_message: None,
            has_unsigned_tx: false,
            has_signed_tx: false,
            mined_height: None,
            chain_tx_id: None,
            fee_microtari: None,
            kernel_excess_sig_nonce: None,
            kernel_excess_sig: None,
            input_count: None,
            total_output_count: None,
            output_commitments: None,
        }],
        payments: Vec::new(),
    };

    summary.record_construction_observations(&snapshot, 100);
    assert!(summary.tx_timings[0]["construction_complete_offset_ms"].is_null());
    assert!(summary.construction_complete_ms.is_empty());

    snapshot.batches[0].has_unsigned_tx = true;
    summary.record_construction_observations(&snapshot, 125);
    summary.record_construction_observations(&snapshot, 150);
    assert_eq!(
        summary.tx_timings[0]["construction_complete_offset_ms"],
        125
    );
    assert_eq!(summary.tx_timings[0]["construction_ms"], 100);
    assert_eq!(
        summary.tx_timings[0]["construction_timing_origin"],
        "payment_processor_db_unsigned_tx_first_observation"
    );
    assert_eq!(summary.construction_complete_ms, vec![125]);
}

#[test]
fn pp_rejection_preserves_structured_batch_identity() {
    let mut summary = PpScenarioSummary::default();
    summary.record_batch(
        1,
        0,
        1,
        vec!["recipient".to_string()],
        Err(payment_processor::PaymentProcessorError::Http {
            status: 500,
            url: "http://localhost/v1/payment-batches".to_string(),
            body: "database busy".to_string(),
            sqlite_code: Some(5),
            request_id: Some("request-1".to_string()),
            batch_id: Some("2f1c928d-9516-48c8-b926-e5cbe33b3168".to_string()),
        }
        .into()),
    );

    let observation = &summary.transaction_observations()[0];
    assert_eq!(observation["terminal_outcome"], "rejected");
    assert_eq!(observation["submission_started"], true);
    assert_eq!(observation["failure_class"], "http_response");
    assert_eq!(observation["http_status"], 500);
    assert_eq!(
        observation["batch_id"],
        "2f1c928d-9516-48c8-b926-e5cbe33b3168"
    );
}

#[test]
fn pp_transport_failure_remains_ambiguous_without_an_explicit_rejection() {
    let mut summary = PpScenarioSummary::default();
    summary.record_batch(
        1,
        0,
        1,
        vec!["recipient".to_string()],
        Err(payment_processor::PaymentProcessorError::Transport {
            message: "connection closed after request write".to_string(),
            request_id: Some("request-1".to_string()),
            batch_id: Some("2f1c928d-9516-48c8-b926-e5cbe33b3168".to_string()),
        }
        .into()),
    );

    let observation = &summary.transaction_observations()[0];
    assert_eq!(observation["terminal_outcome"], "stalled");
    assert_eq!(observation["failure_class"], "transport");
    assert!(observation["api_accepted"].is_null());
    assert_eq!(observation["submission_started"], true);
    assert_eq!(observation["submission_ms"], 1);
    assert_eq!(
        observation["submission_timing_origin"],
        "payment_processor_api_request_ambiguous"
    );
}

#[test]
fn pp_local_pre_api_failure_is_not_faked_as_rejection() {
    let mut summary = PpScenarioSummary::default();
    summary.record_batch(
        1,
        3,
        7,
        vec!["recipient".to_string()],
        Err(anyhow::anyhow!("exact split planner failed")),
    );

    let observation = &summary.transaction_observations()[0];
    assert_eq!(observation["terminal_outcome"], "stalled");
    assert!(observation["api_accepted"].is_null());
    assert_eq!(observation["submission_started"], false);
    assert!(observation["submission_ms"].is_null());
    assert_eq!(observation["failure_class"], "wallet");
    assert_eq!(
        observation["submission_timing_origin"],
        "payment_processor_pre_api"
    );
}

#[test]
fn pp_response_failures_preserve_started_ambiguous_state() {
    let decode_error =
        serde_json::from_str::<serde_json::Value>("{").expect_err("fixture must be invalid JSON");
    let cases = [
        (
            anyhow::Error::from(payment_processor::PaymentProcessorError::Decode {
                url: "http://localhost/v1/payment-batches".to_string(),
                body: "{".to_string(),
                source: decode_error,
                request_id: Some("request-1".to_string()),
                batch_id: None,
            }),
            "response_decode",
        ),
        (
            anyhow::anyhow!("PP batch response missing batch_id"),
            "response_shape",
        ),
    ];
    for (error, expected_class) in cases {
        let mut summary = PpScenarioSummary::default();
        summary.record_batch(1, 2, 9, vec!["recipient".to_string()], Err(error));

        let observation = &summary.transaction_observations()[0];
        assert_eq!(observation["terminal_outcome"], "stalled");
        assert_eq!(observation["submission_started"], true);
        assert_eq!(observation["submission_ms"], 7);
        assert_eq!(observation["failure_class"], expected_class);
        assert_eq!(
            observation["submission_timing_origin"],
            "payment_processor_api_response_ambiguous"
        );
    }
}

#[test]
fn pp_receiver_state_requires_outputs_and_balance_to_converge() {
    assert!(pp_receiver_state_ready(
        Some(2),
        Some(9_999_999_340),
        2,
        Some(9_999_999_340)
    ));
    assert!(!pp_receiver_state_ready(
        Some(2),
        Some(0),
        2,
        Some(9_999_999_340)
    ));
    assert!(!pp_receiver_state_ready(
        Some(0),
        Some(9_999_999_340),
        2,
        Some(9_999_999_340)
    ));
}

#[test]
fn mode2_summary_requires_terminal_verification_for_ok_status() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::NewWallet.as_str().to_string(),
        empty_mode_profile(ModeName::NewWallet, None),
    );
    let summary = ScenarioSendSummary {
        attempted: 1,
        success_count: 1,
        tx_ids: vec!["1".to_string()],
        tx_infos: vec![VerifiedTransaction {
            tx_id: "1".to_string(),
            status_value: 1,
            mode: "new_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: None,
            mined_height: None,
            confirmations: None,
            min_confirmations: None,
            tip_height: None,
            confirmed: false,
        }],
        ..ScenarioSendSummary::default()
    };

    record_mode2_send_summary(&mut profile, ScenarioName::S1, &summary, Vec::new());

    let cell = &profile
        .modes
        .get(ModeName::NewWallet.as_str())
        .unwrap()
        .scenarios[ScenarioName::S1.as_str()];
    assert_eq!(cell.status, CellStatus::Failed);
    assert_eq!(profile.chain_verification.verified_transactions.len(), 0);
    let metrics = cell.repetitions[0].metrics.as_ref().unwrap();
    assert_eq!(
        metrics["observed_transactions"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        metrics["verified_transactions"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn mode2_summary_does_not_emit_unverified_placeholder_rows() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::NewWallet.as_str().to_string(),
        empty_mode_profile(ModeName::NewWallet, None),
    );
    let summary = ScenarioSendSummary {
        attempted: 1,
        success_count: 1,
        tx_ids: vec!["1".to_string()],
        tx_infos: Vec::new(),
        ..ScenarioSendSummary::default()
    };

    record_mode2_send_summary(&mut profile, ScenarioName::S1, &summary, Vec::new());

    let cell = &profile
        .modes
        .get(ModeName::NewWallet.as_str())
        .unwrap()
        .scenarios[ScenarioName::S1.as_str()];
    assert_eq!(cell.status, CellStatus::Failed);
    assert_eq!(profile.chain_verification.verified_transactions.len(), 0);
    assert_eq!(
        cell.repetitions[0].error.as_deref(),
        Some("tx_ids were produced but chain verification returned no rows")
    );
}

#[test]
fn scan_history_commitments_use_transaction_observation_ids() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::NewWallet.as_str().to_string(),
        empty_mode_profile(ModeName::NewWallet, None),
    );
    let cell = profile
        .modes
        .get_mut(ModeName::NewWallet.as_str())
        .unwrap()
        .scenarios
        .get_mut(ScenarioName::S1.as_str())
        .unwrap();
    cell.record_repetition(Repetition {
        run: 1,
        status: CellStatus::Ok,
        wall_ms: Some(1),
        success_count: 1,
        failure_count: 0,
        fee_microtari: Some(1),
        error: None,
        metrics: Some(serde_json::json!({
            "transaction_observations": [{
                "transaction_id": "42",
                "terminal_outcome": "confirmed",
                "output_commitments": ["commitment-1", "commitment-2"]
            }]
        })),
    });

    let commitments =
        verified_history_output_commitments(&profile, "new_wallet", &[ScenarioName::S1]);
    assert_eq!(
        commitments["42"],
        BTreeSet::from(["commitment-1".to_string(), "commitment-2".to_string()])
    );
}

#[test]
fn mode2_db_observed_fallback_stays_out_of_confirmed_rows() {
    let mut summary = ScenarioSendSummary {
        attempted: 1,
        success_count: 1,
        tx_ids: vec!["42".to_string()],
        ..ScenarioSendSummary::default()
    };
    summary.apply_mode2_verification(Mode2VerificationResult {
        observed_transactions: vec![VerifiedTransaction {
            tx_id: "42".to_string(),
            status_value: 1,
            mode: "new_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: Some(10),
            mined_height: None,
            confirmations: None,
            min_confirmations: None,
            tip_height: None,
            confirmed: false,
        }],
        observations: vec![serde_json::json!({
            "tx_id": "42",
            "verification_source": "wallet_db_observed",
            "wallet_db_status": "broadcast",
            "confirmed": false
        })],
        used_base_node_query: false,
        confirmation_observed_offsets_ms: BTreeMap::new(),
        transaction_shapes: BTreeMap::new(),
    });

    let metrics = summary.metrics(ScenarioName::S1);
    assert_eq!(
        metrics["verification_source"],
        serde_json::json!("wallet_db_observed")
    );
    assert_eq!(
        metrics["verification_observations"][0]["wallet_db_status"],
        serde_json::json!("broadcast")
    );
    assert!(summary.verified_transactions().is_empty());
}

#[test]
fn mode2_verification_backfills_confirmed_fee_before_reconciliation() {
    let mut summary = ScenarioSendSummary::default();
    summary.apply_mode2_verification(Mode2VerificationResult {
        observed_transactions: vec![VerifiedTransaction {
            tx_id: "42".to_string(),
            status_value: TX_MINED_CONFIRMED_STATUS,
            mode: "new_wallet".to_string(),
            scenario: ScenarioName::S1.as_str().to_string(),
            amount_microtari: None,
            fee_microtari: Some(660),
            mined_height: Some(100),
            confirmations: Some(3),
            min_confirmations: Some(3),
            tip_height: Some(102),
            confirmed: true,
        }],
        observations: Vec::new(),
        used_base_node_query: true,
        confirmation_observed_offsets_ms: BTreeMap::new(),
        transaction_shapes: BTreeMap::new(),
    });
    assert_eq!(summary.fee_microtari, 660);
}

#[test]
fn mode2_completed_transaction_status_matches_pinned_minotari_strings() {
    assert_eq!(
        mode2_completed_transaction_status("mined_confirmed"),
        (TX_MINED_CONFIRMED_STATUS, true)
    );
    assert_eq!(
        mode2_completed_transaction_status("mined_unconfirmed"),
        (2, false)
    );
    assert_eq!(mode2_completed_transaction_status("broadcast"), (1, false));
    assert_eq!(mode2_completed_transaction_status("completed"), (0, false));
    assert_eq!(mode2_completed_transaction_status("rejected"), (7, false));
    assert_eq!(mode2_completed_transaction_status("canceled"), (14, false));
}

#[test]
fn pp_summary_requires_terminal_verification_for_ok_status() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::PaymentProcessor.as_str().to_string(),
        empty_mode_profile(ModeName::PaymentProcessor, None),
    );
    let summary = PpScenarioSummary {
        attempted_batches: 1,
        attempted_payments: 1,
        accepted_batches: 1,
        batch_ids: vec!["batch".to_string()],
        db_snapshot: Some(PaymentProcessorDbSnapshot {
            batches: vec![payment_processor::PaymentBatchSnapshot {
                id: "batch".to_string(),
                status: "AWAITING_CONFIRMATION".to_string(),
                retry_count: 0,
                error_message: None,
                has_unsigned_tx: true,
                has_signed_tx: true,
                mined_height: None,
                chain_tx_id: None,
                fee_microtari: None,
                kernel_excess_sig_nonce: None,
                kernel_excess_sig: None,
                input_count: None,
                total_output_count: None,
                output_commitments: None,
            }],
            payments: Vec::new(),
        }),
        ..PpScenarioSummary::default()
    };

    record_pp_summary(&mut profile, ScenarioName::S5, &summary, Vec::new());

    let cell = &profile
        .modes
        .get(ModeName::PaymentProcessor.as_str())
        .unwrap()
        .scenarios[ScenarioName::S5.as_str()];
    assert_eq!(cell.status, CellStatus::Failed);
    assert_eq!(profile.chain_verification.verified_transactions.len(), 0);
    assert_eq!(
        cell.repetitions[0].error.as_deref(),
        Some(
            "PP batches were accepted but payment_processor_db_observed returned no confirmed rows"
        )
    );
}

#[test]
fn pp_summary_preflight_failure_cannot_be_reported_as_success() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::PaymentProcessor.as_str().to_string(),
        empty_mode_profile(ModeName::PaymentProcessor, None),
    );
    let summary = PpScenarioSummary {
        failed_batches: 1,
        errors: vec!["could not read spendable inputs".to_string()],
        ..PpScenarioSummary::default()
    };

    record_pp_summary(&mut profile, ScenarioName::S1, &summary, Vec::new());

    let repetition = &profile.modes[ModeName::PaymentProcessor.as_str()].scenarios
        [ScenarioName::S1.as_str()]
    .repetitions[0];
    assert_eq!(repetition.status, CellStatus::Failed);
    assert_eq!(repetition.success_count, 0);
    assert_eq!(repetition.failure_count, 1);
}

#[test]
fn pp_summary_emits_only_confirmed_observed_batches() {
    let config = Config::default();
    let mut profile = ResultProfile::new(&config, crate::env_capture::capture());
    profile.modes.insert(
        ModeName::PaymentProcessor.as_str().to_string(),
        empty_mode_profile(ModeName::PaymentProcessor, None),
    );
    let summary = PpScenarioSummary {
        attempted_batches: 2,
        attempted_payments: 2,
        accepted_batches: 2,
        batch_ids: vec!["confirmed".to_string(), "pending".to_string()],
        chain_proofs: BTreeMap::from([(
            "confirmed".to_string(),
            PpChainProof {
                chain_tx_id: "kernel-confirmed".to_string(),
                amount_microtari: 100,
                fee_microtari: 700,
                mined_height: 42,
                tip_height: 45,
                confirmations: 3,
                min_confirmations: 3,
                shape: TransactionShape {
                    input_count: 1,
                    total_output_count: 2,
                    output_commitments: Vec::new(),
                },
            },
        )]),
        db_snapshot: Some(PaymentProcessorDbSnapshot {
            batches: vec![
                payment_processor::PaymentBatchSnapshot {
                    id: "confirmed".to_string(),
                    status: "CONFIRMED".to_string(),
                    retry_count: 0,
                    error_message: None,
                    has_unsigned_tx: true,
                    has_signed_tx: true,
                    mined_height: Some(42),
                    chain_tx_id: None,
                    fee_microtari: None,
                    kernel_excess_sig_nonce: None,
                    kernel_excess_sig: None,
                    input_count: Some(1),
                    total_output_count: Some(2),
                    output_commitments: Some(Vec::new()),
                },
                payment_processor::PaymentBatchSnapshot {
                    id: "pending".to_string(),
                    status: "PENDING_BATCHING".to_string(),
                    retry_count: 0,
                    error_message: None,
                    has_unsigned_tx: false,
                    has_signed_tx: false,
                    mined_height: None,
                    chain_tx_id: None,
                    fee_microtari: None,
                    kernel_excess_sig_nonce: None,
                    kernel_excess_sig: None,
                    input_count: None,
                    total_output_count: None,
                    output_commitments: None,
                },
            ],
            payments: Vec::new(),
        }),
        ..PpScenarioSummary::default()
    };

    record_pp_summary(&mut profile, ScenarioName::S1, &summary, Vec::new());

    let cell = &profile
        .modes
        .get(ModeName::PaymentProcessor.as_str())
        .unwrap()
        .scenarios[ScenarioName::S1.as_str()];
    assert_eq!(cell.status, CellStatus::Failed);
    assert_eq!(profile.chain_verification.verified_transactions.len(), 1);
    assert_eq!(
        profile.chain_verification.verified_transactions[0].tx_id,
        "kernel-confirmed"
    );
    assert_eq!(cell.repetitions[0].fee_microtari, Some(700));
    let metrics = cell.repetitions[0].metrics.as_ref().unwrap();
    assert_eq!(
        metrics["verification_source"],
        serde_json::json!("payment_processor_db_observed")
    );
}

#[test]
fn mode2_rejected_submission_is_a_failure_with_a_structured_observation() {
    let mut summary = ScenarioSendSummary {
        attempted: 1,
        ..ScenarioSendSummary::default()
    };
    summary.record_attempt(
        1,
        0,
        Some(12),
        "recipient".to_string(),
        Ok(OneSidedSendOutcome {
            tx_id: "rejected-tx".to_string(),
            fee_microtari: 500,
            accepted: false,
            is_synced: true,
            rejection_reason: Some("AlreadyMined".to_string()),
            construction_ms: 12,
            broadcast_to_mempool_ms: None,
            broadcast_start_offset_ms: Some(10),
            broadcasted_at: None,
            recipient: "recipient".to_string(),
        }),
    );

    assert_eq!(summary.success_count, 0);
    assert_eq!(summary.failure_count, 1);
    assert!(summary.tx_ids.is_empty());
    let observations = summary.transaction_observations(ScenarioName::S5);
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0]["terminal_outcome"], "rejected");
    assert_eq!(observations[0]["fee_disposition"], "rejected");
    assert_eq!(observations[0]["error"], "AlreadyMined");
    assert!(observations[0].get("submission_ms").is_some());
}

#[test]
fn summary_notes_distinguish_api_acceptance_from_terminal_outcomes() {
    let observations = vec![
        serde_json::json!({"api_accepted": true, "terminal_outcome": "confirmed"}),
        serde_json::json!({"api_accepted": true, "terminal_outcome": "timed_out"}),
        serde_json::json!({"api_accepted": false, "terminal_outcome": "rejected"}),
    ];
    assert_eq!(
        outcome_note(&observations),
        "outcomes=api_accepted:2 confirmed:1 rejected:1 stalled:0 timed_out:1"
    );
}

#[test]
fn confirmed_transaction_observation_does_not_fake_per_tx_duration() {
    let mut summary = ScenarioSendSummary {
        attempted: 1,
        wall_ms: 321,
        ..ScenarioSendSummary::default()
    };
    summary.record_attempt(
        1,
        0,
        Some(12),
        "recipient".to_string(),
        Ok(OneSidedSendOutcome {
            tx_id: "confirmed-tx".to_string(),
            fee_microtari: 500,
            accepted: true,
            is_synced: true,
            rejection_reason: None,
            construction_ms: 12,
            broadcast_to_mempool_ms: Some(4),
            broadcast_start_offset_ms: Some(10),
            broadcasted_at: None,
            recipient: "recipient".to_string(),
        }),
    );
    summary.tx_infos.push(VerifiedTransaction {
        tx_id: "confirmed-tx".to_string(),
        status_value: TX_MINED_CONFIRMED_STATUS,
        mode: "new_wallet".to_string(),
        scenario: ScenarioName::S5.as_str().to_string(),
        amount_microtari: Some(1_000_000),
        fee_microtari: Some(500),
        mined_height: Some(100),
        confirmations: Some(3),
        min_confirmations: Some(3),
        tip_height: Some(103),
        confirmed: true,
    });

    let observations = summary.transaction_observations(ScenarioName::S5);
    assert_eq!(observations[0]["terminal_outcome"], "confirmed");
    assert_eq!(observations[0]["amount_microtari"], 1_000_000);
    assert_eq!(observations[0]["confirmations"], 3);
    assert_eq!(observations[0]["min_confirmations"], 3);
    assert_eq!(observations[0]["confirmation_ms"], serde_json::Value::Null);
    assert_eq!(
        observations[0]["confirmation_timing_reason"],
        "per-transaction C_min timestamp was not observed; scenario wall time is not substituted"
    );
}

#[test]
fn prepared_transaction_checkpoint_rejects_truncated_json() {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("wallet.db");
    let checkpoint_path = db_path.with_extension("prepared-transaction.json");
    std::fs::write(&checkpoint_path, b"{\"schema_version\":1").unwrap();
    assert!(load_prepared_transaction_checkpoint_at(&checkpoint_path).is_err());
}

#[test]
fn source_wallet_guard_binds_path_address_and_database_fingerprint() {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("source-wallet.db");
    let material = crate::seeds::material_from_seed(
        WalletRole::OldWallet,
        "SOURCE_WALLET_GUARD_TEST".to_string(),
        CipherSeed::random(),
    )
    .unwrap();
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute(
            "CREATE TABLE accounts (\
                 id INTEGER PRIMARY KEY, friendly_name TEXT NOT NULL, fingerprint BLOB NOT NULL)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO accounts (friendly_name, fingerprint) VALUES ('default', ?1)",
            [hex::decode(&material.wallet_fingerprint_hex).unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "CREATE TABLE scanned_tip_blocks (\
                 account_id INTEGER NOT NULL, hash BLOB NOT NULL, height INTEGER NOT NULL)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO scanned_tip_blocks (account_id, hash, height) VALUES (1, ?1, 42)",
            [vec![7u8; 32]],
        )
        .unwrap();
    drop(connection);

    let mut config = Config::default();
    config.source_wallet = Some(crate::config::SourceWalletConfig {
        database: db_path.clone(),
        address: material.address,
        wallet_fingerprint_hex: material.wallet_fingerprint_hex,
    });
    require_dedicated_source_wallet(&config, &db_path).unwrap();
    let anchor = source_wallet_chain_anchor(&db_path).unwrap();
    assert_eq!(
        anchor,
        SourceWalletChainAnchor {
            height: 42,
            hash_hex: "07".repeat(32),
        }
    );
    require_source_wallet_anchor_hashes(&anchor, &"07".repeat(32), &"07".repeat(32)).unwrap();
    assert!(
        require_source_wallet_anchor_hashes(&anchor, &"08".repeat(32), &"07".repeat(32)).is_err()
    );

    let other_path = directory.path().join("other-wallet.db");
    std::fs::write(&other_path, []).unwrap();
    assert!(require_dedicated_source_wallet(&config, &other_path).is_err());
}

#[test]
fn concurrent_transactions_use_distinct_discoverable_checkpoints() {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("wallet.db");
    let first = prepared_transaction_checkpoint_path(&db_path, "1").unwrap();
    let second = prepared_transaction_checkpoint_path(&db_path, "2").unwrap();
    assert_ne!(first, second);
    std::fs::write(&first, b"one").unwrap();
    std::fs::write(&second, b"two").unwrap();
    assert_eq!(
        prepared_transaction_checkpoint_paths(&db_path).unwrap(),
        vec![first, second]
    );
}

#[test]
fn s0_recovery_requires_the_exact_prepared_transaction_binding() {
    let expected = PreparedTransactionBinding {
        purpose: S0_SOURCE_FUNDING_PURPOSE.to_string(),
        b0_run_id: "b0-run".to_string(),
        protocol_fingerprint: "a".repeat(64),
        recipients: vec![
            "old-wallet".to_string(),
            "new-wallet".to_string(),
            "payment-processor".to_string(),
        ],
        amount_microtari_per_recipient: 10_000,
    };
    require_expected_s0_checkpoint_binding(Some(&expected), &expected).unwrap();
    assert!(require_expected_s0_checkpoint_binding(None, &expected).is_err());

    let mut stale = expected.clone();
    stale.b0_run_id = "different-b0-run".to_string();
    assert!(require_expected_s0_checkpoint_binding(Some(&stale), &expected).is_err());
    stale = expected.clone();
    stale.recipients.swap(0, 1);
    assert!(require_expected_s0_checkpoint_binding(Some(&stale), &expected).is_err());
    stale = expected.clone();
    stale.amount_microtari_per_recipient += 1;
    assert!(require_expected_s0_checkpoint_binding(Some(&stale), &expected).is_err());
    stale = expected.clone();
    stale.protocol_fingerprint = "b".repeat(64);
    assert!(require_expected_s0_checkpoint_binding(Some(&stale), &expected).is_err());
}

#[test]
fn only_s0_source_funding_can_resubmit_a_checkpoint() {
    for status in ["completed", "broadcast", "rejected"] {
        assert!(!checkpoint_recovery_submission_allowed(
            CheckpointRecoveryPolicy::ReconcileMeasuredWallet,
            status
        ));
        assert!(!checkpoint_recovery_submission_allowed(
            CheckpointRecoveryPolicy::ReconcileKnownS0Submission,
            status
        ));
    }
    assert!(checkpoint_recovery_submission_allowed(
        CheckpointRecoveryPolicy::RecoverS0SourceFunding,
        "broadcast"
    ));
    assert!(!checkpoint_recovery_submission_allowed(
        CheckpointRecoveryPolicy::RecoverS0SourceFunding,
        "rejected"
    ));
}

#[test]
fn prepared_checkpoint_must_match_the_same_wallet_transaction_row() {
    let row = Mode2CompletedTransactionRow {
        pending_tx_id: "pending".to_string(),
        status: "completed".to_string(),
        mined_height: None,
        confirmation_height: None,
        sent_payref: None,
        serialized_transaction: b"transaction-a".to_vec(),
    };
    require_checkpoint_matches_wallet_row("42", b"transaction-a", Some(&row)).unwrap();
    assert!(require_checkpoint_matches_wallet_row("42", b"transaction-b", Some(&row)).is_err());
    assert!(require_checkpoint_matches_wallet_row("42", b"transaction-a", None).is_err());
}

#[test]
fn mode1_grpc_address_identity_matches_only_the_configured_seed() {
    let material = crate::seeds::material_from_seed(
        WalletRole::OldWallet,
        "WALLET_BENCH_TEST_MODE1".to_string(),
        CipherSeed::random(),
    )
    .unwrap();
    let expected = TariAddress::from_str(&material.address).unwrap().to_vec();

    assert!(mode1_address_matches_seed(&expected, &material).unwrap());
    let mut mismatched = expected;
    mismatched[0] ^= 0x01;
    assert!(!mode1_address_matches_seed(&mismatched, &material).unwrap());
}
