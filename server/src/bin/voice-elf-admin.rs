#[tokio::main]
async fn main() -> anyhow::Result<()> {
    voice_elf_server::run_admin().await
}
