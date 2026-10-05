use {anchor_lang::prelude::Pubkey, mozaik_cctp_forwarder::MESSAGE_TRANSMITTER_ID};

pub const DISCRIMINATOR: [u8; 8] = [131, 100, 133, 56, 166, 225, 151, 60];

const PREFIX: usize = 8 + 32 + 8 + 4;
const HEADER: usize = 148;

#[derive(Debug, PartialEq, Eq)]
pub struct BurnMessage {
    pub rent_payer: Pubkey,
    pub source_domain: u32,
    pub destination_domain: u32,
    pub destination_caller: [u8; 32],
    pub min_finality_threshold: u32,
    pub burn_token: Pubkey,
    pub mint_recipient: [u8; 32],
    pub amount: u64,
    pub message_sender: Pubkey,
    pub max_fee: u64,
    pub body_len: usize,
}

fn bytes32(data: &[u8], at: usize) -> Option<[u8; 32]> {
    data.get(at..at + 32)?.try_into().ok()
}

fn u32_be(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn u256_low(data: &[u8], at: usize) -> Option<u64> {
    let word = bytes32(data, at)?;
    if word[..24].iter().any(|byte| *byte != 0) {
        return None;
    }
    Some(u64::from_be_bytes(word[24..].try_into().ok()?))
}

pub fn is_message_sent(owner: &Pubkey, data: &[u8]) -> bool {
    *owner == MESSAGE_TRANSMITTER_ID && data.starts_with(&DISCRIMINATOR)
}

pub fn parse(owner: &Pubkey, data: &[u8]) -> Option<BurnMessage> {
    if !is_message_sent(owner, data) {
        return None;
    }
    let header = data.get(PREFIX..)?;
    let body = header.get(HEADER..)?;
    Some(BurnMessage {
        rent_payer: Pubkey::new_from_array(bytes32(data, 8)?),
        source_domain: u32_be(header, 4)?,
        destination_domain: u32_be(header, 8)?,
        destination_caller: bytes32(header, 108)?,
        min_finality_threshold: u32_be(header, 140)?,
        burn_token: Pubkey::new_from_array(bytes32(body, 4)?),
        mint_recipient: bytes32(body, 36)?,
        amount: u256_low(body, 68)?,
        message_sender: Pubkey::new_from_array(bytes32(body, 100)?),
        max_fee: u256_low(body, 132)?,
        body_len: body.len(),
    })
}
