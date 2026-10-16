//! Simple wallet transactions give outputs to methods empty, and return values keep size `0` when
//! later inputs refer to them, so methods reject such outputs and inputs of a `TrivialType`, see
//! `TransactionPayloadBuilder::with_method_call()`

// Auto-generated constants will conflict with the main crate when `guest` feature is enabled
#![cfg(not(feature = "guest"))]

use crate::ffi::maybe_data_output::OutputsMaybeDataOutputArgs;
use crate::ffi::plain_input::OutputsPlainInputArgs;
use crate::ffi::plain_output::OutputsPlainOutputArgs;
use crate::ffi::return_value::OutputsReturnValueArgs;
use ab_contracts_common::env::MethodContext;
use ab_contracts_common::{Contract, ContractError};
use ab_contracts_macros::contract;
use ab_contracts_standards::tx_handler::TxHandler;
use ab_core_primitives::address::Address;
use ab_core_primitives::block::BlockRoot;
use ab_core_primitives::shard::ShardIndex;
use ab_core_primitives::transaction::{Gas, Transaction, TransactionHeader};
use ab_example_contract_wallet::{ExampleWallet, ExampleWalletExt};
use ab_executor_native::NativeExecutor;
use ab_io_type::maybe_data::MaybeData;
use ab_io_type::trivial_type::TrivialType;
use ab_system_contract_code::CodeExt;
use ab_system_contract_simple_wallet_base::payload::TransactionMethodContext;
use ab_system_contract_simple_wallet_base::payload::builder::TransactionPayloadBuilder;
use ab_system_contract_simple_wallet_base::seal::hash_and_sign;
use core::mem::MaybeUninit;
use schnorrkel::Keypair;

#[derive(Debug, Copy, Clone, TrivialType)]
#[repr(C)]
pub struct Outputs;

#[contract]
impl Outputs {
    #[view]
    pub fn plain_output(#[output] value: &mut u8) {
        *value = 1;
    }

    #[view]
    pub fn maybe_data_output(#[output] value: &mut MaybeData<u8>) {
        value.replace(1);
    }

    #[view]
    pub fn return_value() -> u8 {
        1
    }

    #[view]
    pub fn plain_input(#[input] &value: &u8) -> Result<(), ContractError> {
        if value == 1 {
            Ok(())
        } else {
            Err(ContractError::Forbidden)
        }
    }
}

/// Deploy a new wallet and `Outputs`, and execute a transaction of the wallet with the method
/// calls `add_method_calls` adds to its payload, given the address of `Outputs`
fn execute<AddMethodCalls>(add_method_calls: AddMethodCalls) -> Result<(), ContractError>
where
    AddMethodCalls: FnOnce(&mut TransactionPayloadBuilder, &Address),
{
    let executor = NativeExecutor::builder(ShardIndex::new(1).unwrap())
        .with_contract::<ExampleWallet>()
        .with_contract_trait::<ExampleWallet, dyn TxHandler>()
        .with_contract::<Outputs>()
        .build()
        .unwrap();
    let slots = &mut executor.new_storage_slots().unwrap();
    let keypair = Keypair::generate();

    let (wallet_address, outputs_address) =
        executor.transaction_emulate(Address::NULL, slots, |env| {
            let wallet_address = env
                .code_deploy(
                    MethodContext::Keep,
                    Address::SYSTEM_CODE,
                    &ExampleWallet::code(),
                )
                .unwrap();
            env.example_wallet_initialize(
                MethodContext::Keep,
                wallet_address,
                &keypair.public.to_bytes(),
            )
            .unwrap();
            let outputs_address = env
                .code_deploy(MethodContext::Keep, Address::SYSTEM_CODE, &Outputs::code())
                .unwrap();

            (wallet_address, outputs_address)
        });

    let header = TransactionHeader {
        version: 0,
        block_root: BlockRoot::default(),
        gas_limit: Gas::default(),
        contract: wallet_address,
    };
    let payload = {
        let mut builder = TransactionPayloadBuilder::default();
        add_method_calls(&mut builder, &outputs_address);
        builder.into_aligned_bytes()
    };
    let read_slots = &[];
    let write_slots = &[];
    let seal = hash_and_sign(&keypair, &header, read_slots, write_slots, &payload, 0);

    executor.transaction_execute(
        Transaction {
            header: &header,
            payload: &payload,
            read_slots,
            write_slots,
            seal: seal.as_bytes(),
        },
        slots,
    )
}

#[test]
fn plain_output() {
    // The output is given empty, while its size must be the size of `u8`
    assert_eq!(
        execute(|builder, outputs_address| {
            builder
                .with_method_call(
                    outputs_address,
                    &OutputsPlainOutputArgs::new(&mut 0),
                    TransactionMethodContext::Null,
                    &[],
                    &[],
                )
                .unwrap();
        }),
        Err(ContractError::BadInput)
    );
}

#[test]
fn maybe_data_output() {
    // The output is given empty, with capacity for `u8`
    assert_eq!(
        execute(|builder, outputs_address| {
            let mut value = 0;
            let mut size = 0;
            builder
                .with_method_call(
                    outputs_address,
                    &OutputsMaybeDataOutputArgs::new(
                        &mut MaybeData::from_mut(&mut value, &mut size).unwrap(),
                    ),
                    TransactionMethodContext::Null,
                    &[],
                    &[],
                )
                .unwrap();
        }),
        Ok(())
    );
}

#[test]
fn return_value_as_plain_input() {
    // The input refers to the return value of the first method, whose size stays `0`, while the
    // size of the input must be the size of `u8`
    assert_eq!(
        execute(|builder, outputs_address| {
            builder
                .with_method_call(
                    outputs_address,
                    &OutputsReturnValueArgs::new(&mut MaybeUninit::uninit()),
                    TransactionMethodContext::Null,
                    &[],
                    &[],
                )
                .unwrap();
            builder
                .with_method_call(
                    outputs_address,
                    &OutputsPlainInputArgs::new(&0),
                    TransactionMethodContext::Null,
                    &[],
                    &[Some(0)],
                )
                .unwrap();
        }),
        Err(ContractError::BadInput)
    );
}
