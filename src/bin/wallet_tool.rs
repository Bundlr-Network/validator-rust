use std::fs;

use clap::{Parser, Subcommand};
use jsonwebkey::{JsonWebKey, Key, PublicExponent, RsaPrivate, RsaPublic};
use openssl::rsa::Rsa;

use validator::key_manager;

#[derive(Subcommand)]
enum Command {
    /// Create new wallet
    Create,
    /// Show Arweaver address for a wallet
    ShowAddress {
        /// Path to JWK file containing Arweaver wallet
        #[clap(short = 'w', long)]
        wallet: String,
    },
}

#[derive(Parser)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

fn main() {
    let args = Args::parse();

    match args.command {
        Command::Create => {
            let rsa = Rsa::generate(2048)
                .expect("Failed to generate enough random data for the private key");

            let jwk = JsonWebKey::new(Key::RSA {
                public: RsaPublic {
                    e: PublicExponent,
                    n: rsa.n().to_vec().into(),
                },
                private: Some(RsaPrivate {
                    d: rsa.d().to_vec().into(),
                    p: rsa.p().map(|v| v.to_vec().into()),
                    q: rsa.q().map(|v| v.to_vec().into()),
                    dp: rsa.dmp1().map(|v| v.to_vec().into()),
                    dq: rsa.dmq1().map(|v| v.to_vec().into()),
                    qi: rsa.iqmp().map(|v| v.to_vec().into()),
                }),
            });

            println!("{}", jwk);
        }
        Command::ShowAddress { ref wallet } => {
            let (_, _, address) = {
                let wallet = fs::read_to_string(wallet).expect("Failed to find wallet file");
                let jwk: JsonWebKey = wallet.parse().expect("Failed to parse wallet file");
                key_manager::split_jwk(&jwk)
            };

            println!(r#"{{"address":"{}"}}"#, address);
        }
    }
}
