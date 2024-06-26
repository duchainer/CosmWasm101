//
// unit testing imports
//
// use crate::contract::{instantiate, query};
// use crate::msg::{ConfigResponse, InstantiateMsg, QueryMsg};

use cosmwasm_std::from_binary;
use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};

//
// integration test imports
//
use cosmwasm_std::{to_binary, Addr, Empty, Uint128};
use cw20::{BalanceResponse, Cw20Coin, Cw20ExecuteMsg, Cw20QueryMsg, MinterResponse};
use cw_multi_test::{App, Contract, ContractWrapper, Executor};

use crate::{
    contract::{execute, instantiate, query},
    msg::{ConfigResponse, Cw20HookMsg, EscrowResponse, ExecuteMsg, InstantiateMsg, QueryMsg},
};

//
// Property testing
//
use proptest::prelude::*;

macro_rules! some_money_amount {
    () => {
        0..999999999999999u128
    };
}

proptest! {
    #[test]
    fn doesnt_crash(
        addrs in vec!["[:ascii:]{3,}", "[:ascii:]{3,}", "[:ascii:]{3,}"],
        values in vec![some_money_amount!(),some_money_amount!(),some_money_amount!(),],
        escrow_time in 10..99999u64,
    ) {
        prop_assume!(addrs[0] != addrs[1]);
        prop_assume!(addrs[1] != addrs[2]);
        prop_assume!(addrs[0] != addrs[2]);

        // TODO Find the better way for that, using proptest facilities
        // The sent amount needs to be less or equal to the sender's account balance.
        let (sent_amount, initial_balances) =
            if values[0] <= values[1]{
                (values[0], vec![ values[1], values[2] ])
            }else{
                (values[1], vec![ values[0], values[2] ])
            };

        prop_assume!(sent_amount != 0); // Invalid zero amount.
        escrow_redeem_is_always_equal_to_send_amount(addrs, initial_balances, sent_amount, escrow_time);
    }
}

fn escrow_redeem_is_always_equal_to_send_amount(
    addrs: Vec<String>,
    initial_balances: Vec<u128>,
    sent_amount: u128,
    escrow_time: u64,
) {
    let owner = Addr::unchecked(addrs[0].clone());
    let alice = Addr::unchecked(addrs[1].clone());
    let bob = Addr::unchecked(addrs[2].clone());

    let mut router: App = App::new(|_, _, _| {});

    // upload the contracts
    let escrow_id = router.store_code(contract_escrow());
    let usdc_id = router.store_code(contract_cw20());

    // instantiate the contracts
    let usdc_addr = router
        .instantiate_contract(
            usdc_id,
            owner.clone(),
            &cw20_base::msg::InstantiateMsg {
                name: "USDC".to_string(),
                symbol: "USDC".to_string(),
                decimals: 9, //see here
                initial_balances: vec![
                    Cw20Coin {
                        address: alice.to_string(),
                        amount: Uint128::from(initial_balances[0]),
                    },
                    Cw20Coin {
                        address: bob.to_string(),
                        amount: Uint128::from(initial_balances[1]),
                    },
                ],
                mint: Some(MinterResponse {
                    minter: owner.to_string(),
                    cap: None,
                }),
                marketing: None,
            },
            &[],
            "cw20",
            None,
        )
        .unwrap();

    let escrow_addr = router
        .instantiate_contract(
            escrow_id,
            owner.clone(),
            &InstantiateMsg {
                token: usdc_addr.to_string(),
            },
            &[],
            "engine",
            None,
        )
        .unwrap();

    // validate the config
    let msg = QueryMsg::Config {};
    let res: ConfigResponse = router
        .wrap()
        .query_wasm_smart(escrow_addr.clone(), &msg)
        .unwrap();
    assert_eq!(res.owner, owner);
    assert_eq!(res.token, usdc_addr.to_string());

    // escrow funds into the contract
    let msg = Cw20ExecuteMsg::Send {
        contract: escrow_addr.to_string(),
        amount: Uint128::from(sent_amount),
        msg: to_binary(&Cw20HookMsg::Escrow { time: escrow_time }).unwrap(),
    };

    let res = router
        .execute_contract(alice.clone(), usdc_addr.clone(), &msg, &[])
        .unwrap();
    assert_eq!("escrow", res.events[3].attributes[1].value);

    // duplicate escrow should fail
    router
        .execute_contract(alice.clone(), usdc_addr.clone(), &msg, &[])
        .unwrap_err();

    // check contract balance
    let msg = Cw20QueryMsg::Balance {
        address: escrow_addr.to_string(),
    };
    let res: BalanceResponse = router
        .wrap()
        .query_wasm_smart(usdc_addr.clone(), &msg)
        .unwrap();
    assert_eq!(res.balance, Uint128::from(sent_amount));

    let msg = QueryMsg::Escrow {
        address: alice.to_string(),
    };
    let res: EscrowResponse = router
        .wrap()
        .query_wasm_smart(escrow_addr.clone(), &msg)
        .unwrap();
    assert_eq!(res.amount, Uint128::from(sent_amount));
    assert_eq!(res.time, 1571797419u64 + escrow_time);

    // redeem funds from the escrow
    let msg = ExecuteMsg::Redeem {};

    // should fail as block has not moved
    router
        .execute_contract(alice.clone(), escrow_addr.clone(), &msg, &[])
        .unwrap_err();

    // move the block time
    router.update_block(|block| {
        block.time = block.time.plus_seconds(escrow_time);
        block.height += 1;
    });

    let res = router
        .execute_contract(alice.clone(), escrow_addr.clone(), &msg, &[])
        .unwrap();
    assert_eq!("redeem", res.events[1].attributes[1].value);

    // check alice balance
    let msg = Cw20QueryMsg::Balance {
        address: alice.to_string(),
    };
    let res: BalanceResponse = router
        .wrap()
        .query_wasm_smart(usdc_addr.clone(), &msg)
        .unwrap();
    assert_eq!(res.balance, Uint128::from(initial_balances[0]));

    // check contract balance
    let msg = Cw20QueryMsg::Balance {
        address: escrow_addr.to_string(),
    };
    let res: BalanceResponse = router
        .wrap()
        .query_wasm_smart(usdc_addr.clone(), &msg)
        .unwrap();
    assert_eq!(res.balance, Uint128::zero());
}

fn contract_cw20() -> Box<dyn Contract<Empty>> {
    let contract = ContractWrapper::new_with_empty(
        cw20_base::contract::execute,
        cw20_base::contract::instantiate,
        cw20_base::contract::query,
    );
    Box::new(contract)
}

fn contract_escrow() -> Box<dyn Contract<Empty>> {
    let contract = ContractWrapper::new_with_empty(execute, instantiate, query);
    Box::new(contract)
}
