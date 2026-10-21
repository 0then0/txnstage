use txnstage::{
    engine::Engine,
    model::{self, Recording, Scenario},
    scenarios, server, worker,
};
#[tokio::main]
async fn main() {
    if let Err(e) = entry().await {
        eprintln!("TxnStage: {e}");
        std::process::exit(1);
    }
}
async fn entry() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let executable = std::env::current_exe()?;
    match args.first().map(String::as_str).unwrap_or("serve") {
        "--worker" => worker::run(
            args.get(1).ok_or("Missing worker path")?,
            serde_json::from_str(args.get(2).ok_or("Missing worker settings")?)?,
        )?,
        "version" | "--version" => {
            println!("{}", serde_json::to_string_pretty(&model::environment())?)
        }
        "scenarios" => {
            let output = args
                .get(1)
                .ok_or("Usage: txnstage scenarios <output-directory>")?;
            std::fs::create_dir_all(output)?;
            for (i, s) in scenarios::builtins().iter().enumerate() {
                std::fs::write(
                    std::path::Path::new(output).join(format!("{:02}.scenario.json", i + 1)),
                    serde_json::to_vec_pretty(s)?,
                )?;
            }
        }
        "run" => {
            let path = args
                .get(1)
                .ok_or("Usage: txnstage run <scenario-or-recording.json> [--output result.json]")?;
            if std::fs::metadata(path)?.len() > model::MAX_ARTIFACT as u64 {
                return Err("Artifact exceeds 8 MiB".into());
            }
            let bytes = std::fs::read(path)?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            let scenario: Scenario = if value.get("scenario").is_some() {
                let recording: Recording = serde_json::from_value(value)?;
                recording.validate()?;
                let current = model::environment();
                for key in ["sqlite_version", "sqlite_source_id", "os", "arch"] {
                    if current[key] != recording.environment[key] {
                        eprintln!(
                            "Environment differs ({key}): recorded {}, current {}",
                            recording.environment[key], current[key]
                        );
                    }
                }
                recording.scenario
            } else {
                serde_json::from_value(value)?
            };
            let engine = Engine::new(scenario, executable).await?;
            let result = tokio::select! {r=engine.replay()=>r,_=tokio::signal::ctrl_c()=>Err("Interrupted by user".into())};
            engine.stop().await;
            let recording = engine.recording().await;
            let output = recording.to_json()?;
            if let Some(i) = args.iter().position(|a| a == "--output") {
                std::fs::write(args.get(i + 1).ok_or("Missing output path")?, output)?;
            } else {
                println!("{output}");
            }
            result?;
        }
        "serve" => {
            let port = if let Some(i) = args.iter().position(|a| a == "--port") {
                args.get(i + 1).ok_or("Missing port")?.parse()?
            } else {
                0
            };
            server::serve(port, executable).await?;
        }
        "help" | "--help" => println!(
            "txnstage [serve [--port <port>]]\ntxnstage run <artifact.json> [--output <recording.json>]\ntxnstage scenarios <output-directory>\ntxnstage version"
        ),
        _ => return Err("Unknown command. Use txnstage help".into()),
    }
    Ok(())
}
