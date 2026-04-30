use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match componentize_qjs_cli::cli::run(args).await {
        Ok(()) => Ok(()),
        Err(err) => {
            if let Some(clap_err) = err.downcast_ref::<clap::Error>() {
                clap_err.exit();
            }
            Err(err)
        }
    }
}
