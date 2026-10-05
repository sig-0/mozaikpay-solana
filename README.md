# MozaikPay Solana

This repository holds the MozaikPay CCTP forwarder for Solana. It moves USDC that arrives at a per-account Solana
deposit address to that account on Base, through Circle's CCTP V2.

## Cluster Reference

| Parameter                       | Solana devnet                                  | Solana mainnet                                 |
|---------------------------------|------------------------------------------------|------------------------------------------------|
| `mozaik_cctp_forwarder` program | `MzkicG4ev5reLESn2RZvPVufghwgwQhRqvhNqVr8oNb`  | `MzkicG4ev5reLESn2RZvPVufghwgwQhRqvhNqVr8oNb`  |
| CCTP V2 TokenMessengerMinterV2  | `CCTPV2vPZJS2u2BBsUoscuikbYjnpFmbFsvVuJdgUMQe` | `CCTPV2vPZJS2u2BBsUoscuikbYjnpFmbFsvVuJdgUMQe` |
| CCTP V2 MessageTransmitterV2    | `CCTPV2Sm4AdWt5296sk4P66VBZ7bEhcARwFaaS9YPbeC` | `CCTPV2Sm4AdWt5296sk4P66VBZ7bEhcARwFaaS9YPbeC` |
| USDC mint                       | `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU` | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` |
| Destination CCTP domain         | `6` (Base Sepolia)                             | `6` (Base)                                     |

The same binary runs on both clusters, because Circle's program ids are the same on both. The program does not pin the
USDC mint. Circle's `local_token` registry binds it, and today the registry of each cluster holds only that cluster's
USDC mint.

## Program Overview

**MozaikCCTPForwarder** (`mozaik_cctp_forwarder`) -- Moves USDC from per-account deposit addresses to the account on
Base. Each account on Base has a forwarder, the PDA with the seeds `["forwarder", account]`, where `account` is the
20-byte Base address. A deposit address is an ordinary Solana wallet whose USDC token account delegates to that
forwarder. The wallet's owner keeps full control of the token account. Anyone can call `forward(account, amount,
max_fee, min_finality_threshold)`. It moves the least of `amount`, the source balance and the delegated amount into a
vault, the forwarder's USDC token account, and fails with `NothingToForward` when that is zero.
