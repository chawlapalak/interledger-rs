mod common;

use bytes::Bytes;
use common::*;
use futures::future::join_all;
use http::StatusCode;
use interledger_api::NodeStore;

use interledger_service::{Account, AccountStore};
use interledger_settlement::core::{
    idempotency::{IdempotentData, IdempotentStore},
    types::{LeftoversStore, SettlementAccount, SettlementStore},
};
use interledger_store::account::AccountId;
use lazy_static::lazy_static;
use num_bigint::BigUint;
use redis::Value;
use redis::{aio::SharedConnection, cmd};
use url::Url;

lazy_static! {
    static ref IDEMPOTENCY_KEY: String = String::from("AJKJNUjM0oyiAN46");
}

#[test]
fn saves_and_gets_uncredited_settlement_amount_properly() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let amounts = vec![
            (BigUint::from(5u32), 11),   // 5
            (BigUint::from(855u32), 12), // 905
            (BigUint::from(1u32), 10),   // 1005 total
        ];
        let acc = AccountId::new();
        let mut f = Vec::new();
        for a in amounts {
            let s = store.clone();
            f.push(s.save_uncredited_settlement_amount(acc, a));
        }
        join_all(f)
            .map_err(|err| eprintln!("Redis error: {:?}", err))
            .and_then(move |_| {
                store
                    .load_uncredited_settlement_amount(acc, 9)
                    .map_err(|err| eprintln!("Redis error: {:?}", err))
                    .and_then(move |ret| {
                        // 1 uncredited unit for scale 9
                        assert_eq!(ret, BigUint::from(1u32));
                        // rest should be in the leftovers store
                        store
                            .get_uncredited_settlement_amount(acc)
                            .map_err(|err| eprintln!("Redis error: {:?}", err))
                            .and_then(move |ret| {
                                // 1 uncredited unit for scale 9
                                assert_eq!(ret, (BigUint::from(5u32), 12));
                                let _ = context;
                                Ok(())
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn clears_uncredited_settlement_amount_properly() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let amounts = vec![
            (BigUint::from(5u32), 11),   // 5
            (BigUint::from(855u32), 12), // 905
            (BigUint::from(1u32), 10),   // 1005 total
        ];
        let acc = AccountId::new();
        let mut f = Vec::new();
        for a in amounts {
            let s = store.clone();
            f.push(s.save_uncredited_settlement_amount(acc, a));
        }
        join_all(f)
            .map_err(|err| eprintln!("Redis error: {:?}", err))
            .and_then(move |_| {
                store
                    .clear_uncredited_settlement_amount(acc)
                    .map_err(|err| eprintln!("Redis error: {:?}", err))
                    .and_then(move |_| {
                        store
                            .get_uncredited_settlement_amount(acc)
                            .map_err(|err| eprintln!("Redis error: {:?}", err))
                            .and_then(move |amount| {
                                assert_eq!(amount, (BigUint::from(0u32), 0));
                                let _ = context;
                                Ok(())
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn credits_prepaid_amount() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context.async_connection().and_then(move |conn| {
            store
                .update_balance_for_incoming_settlement(id, 100, Some(IDEMPOTENCY_KEY.clone()))
                .and_then(move |_| {
                    cmd("HMGET")
                        .arg(format!("accounts:{}", id))
                        .arg("balance")
                        .arg("prepaid_amount")
                        .query_async(conn)
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |(_conn, (balance, prepaid_amount)): (_, (i64, i64))| {
                            assert_eq!(balance, 0);
                            assert_eq!(prepaid_amount, 100);
                            let _ = context;
                            Ok(())
                        })
                })
        })
    }))
    .unwrap()
}

#[test]
fn saves_and_loads_idempotency_key_data_properly() {
    block_on(test_store().and_then(|(store, context, _accs)| {
        let input_hash: [u8; 32] = Default::default();
        store
            .save_idempotent_data(
                IDEMPOTENCY_KEY.clone(),
                input_hash,
                StatusCode::OK,
                Bytes::from("TEST"),
            )
            .map_err(|err| eprintln!("Redis error: {:?}", err))
            .and_then(move |_| {
                store
                    .load_idempotent_data(IDEMPOTENCY_KEY.clone())
                    .map_err(|err| eprintln!("Redis error: {:?}", err))
                    .and_then(move |data1| {
                        assert_eq!(
                            data1.unwrap(),
                            IdempotentData::new(StatusCode::OK, Bytes::from("TEST"), input_hash)
                        );
                        let _ = context;

                        store
                            .load_idempotent_data("asdf".to_string())
                            .map_err(|err| eprintln!("Redis error: {:?}", err))
                            .and_then(move |data2| {
                                assert!(data2.is_none());
                                let _ = context;
                                Ok(())
                            })
                    })
            })
    }))
    .unwrap();
}

#[test]
fn idempotent_settlement_calls() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context.async_connection().and_then(move |conn| {
            store
                .update_balance_for_incoming_settlement(id, 100, Some(IDEMPOTENCY_KEY.clone()))
                .and_then(move |_| {
                    cmd("HMGET")
                        .arg(format!("accounts:{}", id))
                        .arg("balance")
                        .arg("prepaid_amount")
                        .query_async(conn)
                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                        .and_then(move |(conn, (balance, prepaid_amount)): (_, (i64, i64))| {
                            assert_eq!(balance, 0);
                            assert_eq!(prepaid_amount, 100);

                            store
                                .update_balance_for_incoming_settlement(
                                    id,
                                    100,
                                    Some(IDEMPOTENCY_KEY.clone()), // Reuse key to make idempotent request.
                                )
                                .and_then(move |_| {
                                    cmd("HMGET")
                                        .arg(format!("accounts:{}", id))
                                        .arg("balance")
                                        .arg("prepaid_amount")
                                        .query_async(conn)
                                        .map_err(|err| eprintln!("Redis error: {:?}", err))
                                        .and_then(
                                            move |(_conn, (balance, prepaid_amount)): (
                                                _,
                                                (i64, i64),
                                            )| {
                                                // Since it's idempotent there
                                                // will be no state update.
                                                // Otherwise it'd be 200 (100 + 100)
                                                assert_eq!(balance, 0);
                                                assert_eq!(prepaid_amount, 100);
                                                let _ = context;
                                                Ok(())
                                            },
                                        )
                                })
                        })
                })
        })
    }))
    .unwrap()
}

#[test]
fn credits_balance_owed() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(-200)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                id,
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg(format!("accounts:{}", id))
                                    .arg("balance")
                                    .arg("prepaid_amount")
                                    .query_async(conn)
                                    .map_err(|err| panic!(err))
                                    .and_then(
                                        move |(_conn, (balance, prepaid_amount)): (
                                            _,
                                            (i64, i64),
                                        )| {
                                            assert_eq!(balance, -100);
                                            assert_eq!(prepaid_amount, 0);
                                            let _ = context;
                                            Ok(())
                                        },
                                    )
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn withdraw_funds_fails_negative_balance() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HMSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(-200)
                    .arg("prepaid_amount")
                    .arg(199)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(_, _): (SharedConnection, Value)| {
                        // Fails because negative balance
                        store.withdraw_funds(id, 100).and_then(move |_| {
                            let _ = context;
                            Ok(())
                        })
                    })
            })
    }))
    .unwrap_err()
}

#[test]
fn withdraw_funds_fails_requested_too_much() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HMSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(100)
                    .arg("prepaid_amount")
                    .arg(100)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(_, _): (SharedConnection, Value)| {
                        // Fails because 100+100=200, and we requested 201 which is more than that
                        store.withdraw_funds(id, 201).and_then(move |_| {
                            let _ = context;
                            Ok(())
                        })
                    })
            })
    }))
    .unwrap_err()
}

#[test]
fn withdraw_funds_fails_more_than_min_balance() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HMSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(100)
                    .arg("prepaid_amount")
                    .arg(100)
                    .arg("min_balance")
                    .arg(100)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(_, _): (SharedConnection, Value)| {
                        // Fails because 100+100-100=100, and we requested 101 which is more than that
                        store.withdraw_funds(id, 101).and_then(move |_| {
                            let _ = context;
                            Ok(())
                        })
                    })
            })
    }))
    .unwrap_err()
}

#[test]
fn withdraw_funds_prepaid_more_than_requested() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HMSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(100)
                    .arg("prepaid_amount")
                    .arg(105)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _): (SharedConnection, Value)| {
                        // the prepaid amount is sufficient so the balance should be untouched
                        store.withdraw_funds(id, 100).and_then(move |_| {
                            cmd("HMGET")
                                .arg(format!("accounts:{}", id))
                                .arg("balance")
                                .arg("prepaid_amount")
                                .query_async(conn)
                                .map_err(|err| panic!(err))
                                .and_then(
                                    move |(_, (balance, prepaid_amount)): (
                                        SharedConnection,
                                        (i64, i64),
                                    )| {
                                        assert_eq!(balance, 100);
                                        assert_eq!(prepaid_amount, 5);
                                        let _ = context;
                                        Ok(())
                                    },
                                )
                        })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn withdraw_funds_prepaid_bigger_than_zero() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HMSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(100)
                    .arg("prepaid_amount")
                    .arg(5)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _): (SharedConnection, Value)| {
                        // the prepaid amount is sufficient so the balance should be untouched
                        store.withdraw_funds(id, 100).and_then(move |_| {
                            cmd("HMGET")
                                .arg(format!("accounts:{}", id))
                                .arg("balance")
                                .arg("prepaid_amount")
                                .query_async(conn)
                                .map_err(|err| panic!(err))
                                .and_then(
                                    move |(_, (balance, prepaid_amount)): (
                                        SharedConnection,
                                        (i64, i64),
                                    )| {
                                        assert_eq!(balance, 5);
                                        assert_eq!(prepaid_amount, 0);
                                        let _ = context;
                                        Ok(())
                                    },
                                )
                        })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn withdraw_funds_prepaid_zero() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HMSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(100)
                    .arg("prepaid_amount")
                    .arg(0)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _): (SharedConnection, Value)| {
                        // the prepaid amount is sufficient so the balance should be untouched
                        store.withdraw_funds(id, 99).and_then(move |_| {
                            cmd("HMGET")
                                .arg(format!("accounts:{}", id))
                                .arg("balance")
                                .arg("prepaid_amount")
                                .query_async(conn)
                                .map_err(|err| panic!(err))
                                .and_then(
                                    move |(_, (balance, prepaid_amount)): (
                                        SharedConnection,
                                        (i64, i64),
                                    )| {
                                        assert_eq!(balance, 1);
                                        assert_eq!(prepaid_amount, 0);
                                        let _ = context;
                                        Ok(())
                                    },
                                )
                        })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn clears_balance_owed() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(-100)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                id,
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg(format!("accounts:{}", id))
                                    .arg("balance")
                                    .arg("prepaid_amount")
                                    .query_async(conn)
                                    .map_err(|err| panic!(err))
                                    .and_then(
                                        move |(_conn, (balance, prepaid_amount)): (
                                            _,
                                            (i64, i64),
                                        )| {
                                            assert_eq!(balance, 0);
                                            assert_eq!(prepaid_amount, 0);
                                            let _ = context;
                                            Ok(())
                                        },
                                    )
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn clears_balance_owed_and_puts_remainder_as_prepaid() {
    block_on(test_store().and_then(|(store, context, accs)| {
        let id = accs[0].id();
        context
            .shared_async_connection()
            .map_err(|err| panic!(err))
            .and_then(move |conn| {
                cmd("HSET")
                    .arg(format!("accounts:{}", id))
                    .arg("balance")
                    .arg(-40)
                    .query_async(conn)
                    .map_err(|err| panic!(err))
                    .and_then(move |(conn, _balance): (SharedConnection, i64)| {
                        store
                            .update_balance_for_incoming_settlement(
                                id,
                                100,
                                Some(IDEMPOTENCY_KEY.clone()),
                            )
                            .and_then(move |_| {
                                cmd("HMGET")
                                    .arg(format!("accounts:{}", id))
                                    .arg("balance")
                                    .arg("prepaid_amount")
                                    .query_async(conn)
                                    .map_err(|err| panic!(err))
                                    .and_then(
                                        move |(_conn, (balance, prepaid_amount)): (
                                            _,
                                            (i64, i64),
                                        )| {
                                            assert_eq!(balance, 0);
                                            assert_eq!(prepaid_amount, 60);
                                            let _ = context;
                                            Ok(())
                                        },
                                    )
                            })
                    })
            })
    }))
    .unwrap()
}

#[test]
fn loads_globally_configured_settlement_engine_url() {
    block_on(test_store().and_then(|(store, context, accs)| {
        assert!(accs[0].settlement_engine_details().is_some());
        assert!(accs[1].settlement_engine_details().is_none());
        let account_ids = vec![accs[0].id(), accs[1].id()];
        store
            .clone()
            .get_accounts(account_ids.clone())
            .and_then(move |accounts| {
                assert!(accounts[0].settlement_engine_details().is_some());
                assert!(accounts[1].settlement_engine_details().is_none());

                store
                    .clone()
                    .set_settlement_engines(vec![
                        (
                            "ABC".to_string(),
                            Url::parse("http://settle-abc.example").unwrap(),
                        ),
                        (
                            "XYZ".to_string(),
                            Url::parse("http://settle-xyz.example").unwrap(),
                        ),
                    ])
                    .and_then(move |_| {
                        store.get_accounts(account_ids).and_then(move |accounts| {
                            // It should not overwrite the one that was individually configured
                            assert_eq!(
                                accounts[0]
                                    .settlement_engine_details()
                                    .unwrap()
                                    .url
                                    .as_str(),
                                "http://settlement.example/"
                            );

                            // It should set the URL for the account that did not have one configured
                            assert!(accounts[1].settlement_engine_details().is_some());
                            assert_eq!(
                                accounts[1]
                                    .settlement_engine_details()
                                    .unwrap()
                                    .url
                                    .as_str(),
                                "http://settle-abc.example/"
                            );
                            let _ = context;
                            Ok(())
                        })
                    })
                // store.set_settlement_engines
            })
    }))
    .unwrap()
}
