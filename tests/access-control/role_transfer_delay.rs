//! Role Transfer Acceptance Delay Integration Tests (#507)
//!
//! Privileged role transfers in the payroll contract are two-step: the current
//! holder proposes a successor, and the successor must explicitly accept. This
//! suite pins the acceptance delay added in #507: the recipient cannot activate
//! a transferred role until the window has elapsed, and the transfer stays
//! cancellable throughout.
//!
//! The views used here (`get_admin_transfer_status`, ...) are privacy-safe: they
//! expose only the role, the candidate address, the delay and ledger
//! timestamps -- never salary amounts, commitments, or employee data.
//!
//! ## How to run
//!
//! ```bash
//! cargo test -p access_control_tests --test role_transfer_delay
//! ```

use payroll::{Payroll, PayrollClient, ROLE_TRANSFER_ACCEPTANCE_DELAY};
use proof_verifier::{ProofVerifier, ProofVerifierClient, VerificationKey};
use salary_commitment::{SalaryCommitmentContract, SalaryCommitmentContractClient};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, BytesN, Env, Symbol, Vec};
use token::Token;

fn mock_vk(env: &Env) -> VerificationKey {
    VerificationKey {
        alpha: BytesN::from_array(env, &[0u8; 64]),
        beta: BytesN::from_array(env, &[0u8; 128]),
        gamma: BytesN::from_array(env, &[0u8; 128]),
        delta: BytesN::from_array(env, &[0u8; 128]),
        ic: Vec::from_array(
            env,
            [
                BytesN::from_array(env, &[0u8; 64]),
                BytesN::from_array(env, &[0u8; 64]),
                BytesN::from_array(env, &[0u8; 64]),
                BytesN::from_array(env, &[0u8; 64]),
            ],
        ),
    }
}

fn setup_payroll(env: &Env) -> (PayrollClient<'_>, Address, Address) {
    env.mock_all_auths();

    let verifier_id = env.register_contract(None, ProofVerifier);
    let verifier_client = ProofVerifierClient::new(env, &verifier_id);
    let verifier_admin = Address::generate(env);
    verifier_client.init_verifier_admin(&verifier_admin);
    verifier_client.initialize_verifier(&mock_vk(env));

    let commitment_id = env.register_contract(None, SalaryCommitmentContract);
    let commitment_client = SalaryCommitmentContractClient::new(env, &commitment_id);
    let commitment_admin = Address::generate(env);
    commitment_client.init_commitment_admin(&commitment_admin);

    let token_id = env.register_contract(None, Token);

    let payroll_id = env.register_contract(None, Payroll);
    let payroll_client = PayrollClient::new(env, &payroll_id);

    let treasury = Address::generate(env);
    let admin = Address::generate(env);
    let treasury_owner = Address::generate(env);

    payroll_client.initialize(
        &admin,
        &token_id,
        &verifier_id,
        &commitment_id,
        &treasury,
        &treasury_owner,
    );

    (payroll_client, admin, treasury_owner)
}

#[test]
fn test_role_transfer_activates_only_after_acceptance_delay() {
    let env = Env::default();
    let (client, admin, _treasury_owner) = setup_payroll(&env);

    let successor = Address::generate(&env);
    client.propose_admin_rotation(&admin, &successor);

    let status = client
        .get_admin_transfer_status()
        .expect("pending admin transfer must be visible");
    assert_eq!(status.role, Symbol::new(&env, "admin"));
    assert_eq!(status.new_holder, successor);
    assert_eq!(status.delay_seconds, ROLE_TRANSFER_ACCEPTANCE_DELAY);
    assert_eq!(
        status.ready_at,
        status.proposed_at + ROLE_TRANSFER_ACCEPTANCE_DELAY
    );

    // One second short of the window: the role is still held by the proposer.
    env.ledger().set_timestamp(status.ready_at - 1);
    assert!(
        client.try_accept_admin_rotation(&successor).is_err(),
        "accepting before the acceptance delay must fail"
    );
    assert!(
        client.get_pending_admin_rotation().is_some(),
        "a rejected accept must leave the proposal pending"
    );

    // At the window the successor holds the admin role.
    env.ledger().set_timestamp(status.ready_at);
    client.accept_admin_rotation(&successor);
    assert!(client.get_pending_admin_rotation().is_none());
    assert!(client.get_admin_transfer_status().is_none());

    let draft_id = client.create_run_draft(&successor, &1_000i128, &1u32, &Symbol::new(&env, "Q1"));
    assert_eq!(
        draft_id, 1,
        "the new admin must be able to run admin operations"
    );
}

#[test]
fn test_role_transfer_delay_covers_treasury_and_handover() {
    let env = Env::default();
    let (client, admin, treasury_owner) = setup_payroll(&env);

    let new_owner = Address::generate(&env);
    client.propose_treasury_rotation(&treasury_owner, &new_owner);

    let successor_admin = Address::generate(&env);
    client.request_admin_handover(&admin, &successor_admin);

    assert!(
        client.try_accept_treasury_rotation(&new_owner).is_err(),
        "treasury role must wait for the acceptance delay"
    );
    assert!(
        client.try_accept_admin_handover(&successor_admin).is_err(),
        "admin handover must wait for the acceptance delay"
    );

    env.ledger().set_timestamp(ROLE_TRANSFER_ACCEPTANCE_DELAY);
    client.accept_treasury_rotation(&new_owner);
    client.accept_admin_handover(&successor_admin);

    assert!(client.get_pending_treasury_rotation().is_none());
    assert!(client.get_pending_admin_handover().is_none());
}

#[test]
fn test_role_transfer_cancelled_inside_delay_window() {
    let env = Env::default();
    let (client, admin, _treasury_owner) = setup_payroll(&env);

    let successor = Address::generate(&env);
    client.propose_admin_rotation(&admin, &successor);

    env.ledger()
        .set_timestamp(ROLE_TRANSFER_ACCEPTANCE_DELAY - 1);
    client.cancel_admin_rotation(&admin);
    assert!(client.get_pending_admin_rotation().is_none());

    // Even once the original window opens, the cancelled proposal is dead.
    env.ledger()
        .set_timestamp(ROLE_TRANSFER_ACCEPTANCE_DELAY * 2);
    assert!(
        client.try_accept_admin_rotation(&successor).is_err(),
        "a cancelled transfer must never become active"
    );
}
