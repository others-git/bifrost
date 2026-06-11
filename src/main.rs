#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bifrost::run().await
}
