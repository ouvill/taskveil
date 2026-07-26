use taskveil_client::{LocalProfileConfig, TaskveilClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_dir = std::env::args_os()
        .nth(1)
        .ok_or("usage: seed_home_calendar_performance_fixture <db-dir>")?;
    std::env::set_var("FLUTTER_TEST", "1");

    let client = TaskveilClient::open(LocalProfileConfig::new(db_dir, "Inbox"))?;
    let count = client.seed_home_calendar_performance_fixture()?;
    println!("seeded {count} SQLCipher tasks");
    Ok(())
}
