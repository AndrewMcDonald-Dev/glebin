use clap::{Arg, Command};

fn main() {
    // CLI command for connecting to a server from command line
    let matches = Command::new("glebin")
        .version("0.1")
        .author("Andrew McDonald")
        .about("A simple client for the glebin")
        .arg(
            Arg::new("connect")
                .short('c')
                .long("connect")
                //.takes_value(true)
                .value_name("HOST:PORT")
                .help("Connect to a glebin server"),
        )
        .get_matches();

    match matches.get_one::<String>("connect") {
        Some(host_port) => {
            println!("Connecting to {}", host_port);
        }
        None => {
            println!("No host:port specified");
        }
    }
}
