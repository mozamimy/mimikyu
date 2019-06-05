pub mod server;

fn main() {
    match initialize_logger() {
        Ok(_) => { /* do nothing */ }
        Err(e) => {
            eprintln!("Failed to initialize logger.");
            eprintln!("{:?}", e);
            std::process::exit(1);
        }
    }

    let matches = clap::App::new("mimikyu")
        .version("0.1.0")
        .author("mozamimy <alice@mozami.me>")
        .arg(
            clap::Arg::with_name("PRIMARY_CONFIGURATION_ENDPOINT")
                .required(true)
                .index(1),
        )
        .arg(
            clap::Arg::with_name("SECONDARY_CONFIGURATION_ENDPOINT")
                .required(true)
                .index(2),
        )
        .get_matches();
    let primary_endpoint = matches.value_of("PRIMARY_CONFIGURATION_ENDPOINT").unwrap();
    let secondary_endpoint = matches
        .value_of("SECONDARY_CONFIGURATION_ENDPOINT")
        .unwrap();

    let server = match server::Server::new(primary_endpoint, secondary_endpoint) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to initialize server.");
            log::error!("{:?}", e);
            std::process::exit(1);
        }
    };

    match server.listen() {
        Ok(_) => { /* do nothing */ }
        Err(e) => {
            log::error!("{:?}", e);
            std::process::exit(1);
        }
    }

    log::info!("Bye bye...");
}

fn initialize_logger() -> Result<(), failure::Error> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::try_init()?;

    Ok(())
}
