# Security Policy

The MozaikPay CCTP forwarder on Solana moves real value: it burns USDC through Circle's CCTP V2 so that it mints to an
account on Base. We take security seriously and appreciate the work of researchers who help keep users' funds safe.
This document explains what is in scope, how to report a vulnerability privately, and what to expect from us in return.

## Reporting a Vulnerability

**Please do not open a public issue, pull request, or discussion for a security vulnerability.** Public disclosure
before a fix is deployed puts user funds at risk.

Report privately through **GitHub's private vulnerability reporting**:

1. Go to the [**Security** tab](https://github.com/sig-0/mozaikpay-solana/security)
   of this repository.
2. Click **Report a vulnerability** to open a private advisory visible only to you and the maintainers.

If you cannot use GitHub private reporting for any reason, contact the maintainers listed in
[`.github/CODEOWNERS`](.github/CODEOWNERS) and request a secure channel before sharing any details.

### What to include

A good report lets us reproduce and assess impact quickly. Please include:

- **The affected instruction** and the version (Git commit hash, or program id and cluster).
- **A description of the vulnerability** and the security property it breaks.
- **Impact**: what an attacker can achieve (e.g. move USDC from a deposit address to anything but its account on Base,
  burn to another mint recipient, take the rent of an account).
- **A proof of concept** where possible: a LiteSVM test in `programs/mozaik-cctp-forwarder/tests/`, a transaction trace, or
  step-by-step reproduction. PoCs against LiteSVM or Solana **devnet** are strongly preferred over mainnet (see Safe
  Harbor).
- **Suggested remediation**, if you have one.

We can review reports in English.

## Scope

### In scope

The production Rust in this repository (`programs/`) and the program deployed from it:

| Program                 | Path                              | Program id                                    |
|-------------------------|-----------------------------------|-----------------------------------------------|
| `mozaik_cctp_forwarder` | `programs/mozaik-cctp-forwarder/` | `MzkicG4ev5reLESn2RZvPVufghwgwQhRqvhNqVr8oNb` |

**Clusters:** Solana mainnet and Solana devnet. The program has the same id on both.

Examples of in-scope issues:

- Any path that moves USDC out of a token account that delegates to a forwarder other than a CCTP V2 burn whose mint
  recipient is that forwarder's account on Base.
- Any way to make `forward` burn to another destination domain, set a destination caller or hook data, or exceed the
  20 bps fee cap.
- Any way to sign as a forwarder outside `forward`, or to use one account's forwarder for another account.
- Griefing that makes `forward` fail permanently for a deposit address that holds USDC, other than the burn-limit case
  under Out of scope.

### Out of scope

- **Circle's CCTP V2 programs, the SPL Token program and the associated token account program**: these are external
  and not under our control. Report those upstream. (In scope if we use them in an unsafe or incorrect way.)
- **Circle's roles**: Circle's pauser, denylister, attesters, token_controller, min_fee_controller and the upgrade
  authorities of its programs can each stop forwarding. The 20 bps fee cap is final for this program id, so a Circle
  `min_fee` above 20 bps stops every forward, Standard included. An upgraded TokenMessengerMinterV2 could also take USDC
  that rests at deposit addresses, because `forward` signs into it as the forwarder, the delegate of those token
  accounts. On Solana, Circle's denylist checks only the burn owner, which is the forwarder. It checks neither the
  caller nor the deposit address, so one entry stops forwarding from every deposit address of that account.
- **Burn-limit stuffing**: anyone can park at least Circle's `burn_limit_per_message` in a forwarder's vault, its USDC
  token account. Every later `forward` for that account then burns more than the limit and fails until Circle raises
  it. The parked USDC stays locked for the account, so the attacker forfeits it. A Circle cut of the limit below a
  stray vault balance has the same effect.
- **Dust splitting**: anyone can forward small amounts at a time from a funded deposit address. Each burn still mints
  to the account on Base, and the caller pays the fees.
- **Assets sent to a forwarder outside its USDC token account**: SOL sent to the forwarder, a non-associated USDC
  token account that the forwarder owns, and tokens of a mint that Circle does not register stay there for good. USDC
  that a wallet sends to the forwarder goes into its vault, and the next `forward` burns it to the account.
- **The owner key of a deposit address**: the owner of a token account can always revoke its delegate or move its
  balance. That key belongs to the account holder by design.
- **Off-chain and operational infrastructure**: callers of `forward`, RPC providers and applications. These live in
  other repositories.
- **Devnet-only issues** with no mainnet impact, compute-unit optimization suggestions, and best-practice/style
  recommendations without a demonstrated security impact.
- Automated scanner output submitted without analysis or a plausible exploit.

## Supported Versions

The security-supported code is:

- The **`main`** branch of this repository, and
- The program **currently deployed** on Solana mainnet from this repository.

The upgrade authority of the program is removed after its review, so the deployed program cannot change. A fix ships
as a new program at a new program id, and funds at deposit addresses that delegate to a forwarder of the old program leave only
through that program's own paths. Remediation for a live issue may involve stopping the use of the old program and
coordinating with you under embargo.

## Our Commitment / Response Process

When you report through GitHub private reporting, you can expect:

| Stage                                                        | Target                                                |
|--------------------------------------------------------------|-------------------------------------------------------|
| **Acknowledgement** of your report                           | within **3 business days**                            |
| **Initial assessment** (severity + whether we can reproduce) | within **7 business days**                            |
| **Status updates** while we work a confirmed issue           | at least every **7 days**                             |
| **Fix / mitigation** for confirmed Critical & High issues    | as quickly as practical, prioritized above other work |

We will keep you informed through the advisory, credit you in the published advisory unless you prefer to remain
anonymous, and coordinate public disclosure timing with you.

## Severity

We assess severity by realistic impact on funds and users, roughly:

- **Critical**: direct theft or permanent loss/freezing of USDC at deposit addresses; a burn to another recipient.
- **High**: theft/loss under specific but attainable conditions; griefing that locks funds.
- **Medium**: limited-value loss, or an exploit requiring unlikely preconditions.
- **Low**: minimal impact, or issues that are defense-in-depth improvements.

## Coordinated Disclosure

We follow coordinated disclosure. Please give us a reasonable window to investigate and remediate before any public
disclosure. **90 days** from the acknowledgement is our default, but for a live, actively-exploitable issue that
threatens funds we may ask for an extension while a fix is rolled out. We will publish a GitHub Security Advisory once
a fix is deployed and agree disclosure timing with you.

## Safe Harbor

We will not pursue or support legal action against researchers who, in good faith:

- Report vulnerabilities through the private channel above and give us reasonable time to respond before disclosure;
- Make a good-faith effort to avoid privacy violations, data destruction, and interruption or degradation of our
  services;
- Test against **LiteSVM or Solana devnet**, not against mainnet user funds, and do **not** exploit an issue beyond
  the minimum needed to demonstrate it;
- Do not access, modify, or exfiltrate data that is not their own.

If in doubt about whether an action is authorized, contact us and ask first.

## Rewards

We do not currently run a formal bug bounty program. Rewards may be offered at the maintainers' discretion for
high-impact reports, and we will publicly credit researchers (with your consent) in the resulting advisory. We genuinely
appreciate responsible disclosure and the time you invest.

## Security Posture

For context on the assurances already in place, CI runs these gates on every pull request and every push to `main`.

- LiteSVM tests against the deployed CCTP V2 programs, accounts and feature gates of Solana devnet and Solana mainnet.
- End-to-end tests on local validators that mirror both clusters.
- Stateful invariant fuzzing with Crucible against the devnet and mainnet fixtures.
- Mutation testing with cargo-mutants. A surviving mutant fails the run.
- cargo-deny for advisories, licenses, bans and sources.
- An IDL drift check against a fresh build.
- Radar static analysis. A high or critical finding fails the run.
- rustfmt, and Clippy with warnings denied.

The program embeds a [`security.txt`](https://github.com/neodyme-labs/solana-security-txt) that points to this policy.
These reduce, but do not eliminate, risk. Independent findings are always welcome.

---

Thank you for helping keep MozaikPay users safe.
