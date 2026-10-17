//! Small-model batch driver: isolated native CLI process per item, resumable studio ledger.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo, clippy::unimplemented, unsafe_code)]
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mode = args.first().and_then(|v| v.to_str()).ok_or("usage: shorts-manager render|schedule|run MANIFEST.json")?;
    if !["render", "schedule", "run"].contains(&mode) {
        return Err("mode must be render, schedule, or run".into());
    }
    let path = PathBuf::from(args.get(1).ok_or("manifest path required")?);
    if std::fs::metadata(&path)?.len() > 2 * 1024 * 1024 {
        return Err("manifest exceeds 2 MiB".into());
    }
    let manifest: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    if manifest["version"] != 1 {
        return Err("manifest version must be 1".into());
    }
    let items = manifest["items"].as_array().filter(|a| a.len() <= 100).ok_or("items must be an array of at most 100")?;
    let cli = std::env::current_exe()?.with_file_name("shorts-manager-engine.exe");
    let mut results = Vec::new();
    for item in items {
        let plan: filmcraft_studio::workflow::Editorial = serde_json::from_value(item["plan"].clone())?;
        let identity = json!({"channel":plan.channel,"asset":plan.asset});
        let mut steps = Vec::new();
        if mode != "schedule" {
            steps.push(json!({"id":"studio.workflowPlan","params":item["plan"]}));
            steps.push(json!({"id":"studio.workflowRender","params":identity}));
        }
        if mode != "render" {
            let account = item["account"].as_str().ok_or("each delivery needs cloud account")?;
            let at = item["at"].as_str().ok_or("each delivery needs UTC at")?;
            steps.push(json!({"id":"studio.workflowSchedule","params":{"channel":plan.channel,"asset":plan.asset,"account":account,"at":at}}));
        }
        let log = path.with_file_name(format!("asset-{}-{mode}.jsonl", plan.asset));
        let log_file = std::fs::File::create(&log)?;
        let mut child = Command::new(&cli).args(["run", "-"]).stdin(Stdio::piped()).stdout(Stdio::from(log_file)).stderr(Stdio::null()).spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            for step in steps {
                writeln!(stdin, "{step}")?;
            }
        }
        let start = Instant::now();
        let exit = loop {
            if let Some(code) = child.try_wait()? {
                break Some(code);
            }
            if start.elapsed() > Duration::from_secs(7200) {
                child.kill()?;
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(250));
        };
        let mut last = Value::Null;
        let mut ok = exit.is_some_and(|e| e.success());
        for line in std::fs::read_to_string(&log)?.lines() {
            let v: Value = serde_json::from_str(line)?;
            if v["ok"] != true {
                ok = false;
            }
            last = v;
        }
        let result = json!({"asset":plan.asset,"ok":ok,"mode":mode,"last":last,"log":log});
        writeln!(std::io::stdout(), "{result}")?;
        results.push(result);
        let report = path.with_file_name(format!("results-{mode}.json"));
        let temp = report.with_extension("tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&results)?)?;
        std::fs::rename(&temp, &report)?;
    }
    if results.iter().any(|r| r["ok"] != true) {
        return Err("one or more items failed; inspect results and resume using the same manifest".into());
    }
    Ok(())
}
fn main() {
    let result = std::panic::catch_unwind(run);
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = writeln!(std::io::stderr(), "shorts-manager: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            let _ = writeln!(std::io::stderr(), "shorts-manager: internal failure; inspect ledger before retry");
            std::process::exit(1);
        }
    }
}
