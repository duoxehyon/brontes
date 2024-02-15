use std::fmt::Debug;

use clickhouse::Row;
use malachite::Rational;
use reth_primitives::Address;
use serde::{Deserialize, Serialize};

use crate::db::token_info::TokenInfoWithAddress;

#[derive(Debug, Serialize, Clone, Row, PartialEq, Eq, Deserialize)]
pub struct NormalizedTransfer {
    pub trace_index: u64,
    pub from: Address,
    pub to: Address,
    pub token: TokenInfoWithAddress,
    pub amount: Rational,
    pub fee: Rational,
}
