use {
    anchor_lang::solana_program::{program_option::COption, program_pack::Pack},
    anchor_spl::token::spl_token::state::Mint,
    base64::{Engine, engine::general_purpose::STANDARD},
    std::{env, error::Error, fs},
};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    let [_, path, authority] = args.as_slice() else {
        return Err("usage: usdc_mint <mint.json> <mint authority>".into());
    };

    let mut dump: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let data = &mut dump["account"]["data"][0];
    let encoded = data.as_str().ok_or("the dump has no base64 data")?;

    let mut mint = Mint::unpack(&STANDARD.decode(encoded)?)?;
    mint.mint_authority = COption::Some(authority.parse()?);

    let mut bytes = vec![0u8; Mint::LEN];
    Mint::pack(mint, &mut bytes)?;
    *data = STANDARD.encode(bytes).into();

    println!("{dump}");
    Ok(())
}
