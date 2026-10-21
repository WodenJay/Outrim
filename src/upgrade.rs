mod background;
mod release;
pub use background::apply;
mod transaction;

use outrim::install::Agent;
use std::{fs, io, path::Path, process::Command};
use transaction::{Plan, Target};

pub fn render(executable: &Path) -> io::Result<()> {
    let templates = [
        Agent::Pi.render(executable)?,
        Agent::OpenCode.render(executable)?,
    ];
    println!(
        "{}",
        serde_json::to_string(&templates).map_err(io::Error::other)?
    );
    Ok(())
}

pub fn run(pi: &Path, opencode: &Path) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let Some(task) = background::Task::acquire(&executable)? else {
        return Ok(());
    };
    println!("Checking binary: {}", executable.display());
    let mut plan = Plan::default();
    plan.targets
        .push(Target::inspect(executable.clone(), None)?);
    for (agent, directory) in [(Agent::Pi, pi), (Agent::OpenCode, opencode)] {
        let path = agent.path(directory);
        println!("Checking {}: {}", agent.name(), path.display());
        if agent.read(&path)?.is_some() {
            plan.targets.push(Target::inspect(path, Some(agent))?);
        }
    }
    let work = task.workspace()?;
    let result = (|| {
        let downloaded = release::prepare(work.path(), &executable)?;
        let templates: [String; 2] = if let Some(binary) = &downloaded {
            let output = Command::new(binary)
                .arg("__upgrade-render")
                .arg(&executable)
                .output()?;
            if !output.status.success() {
                return Err(io::Error::other(
                    "target binary could not render integrations",
                ));
            }
            serde_json::from_slice(&output.stdout).map_err(io::Error::other)?
        } else {
            [
                Agent::Pi.render(&executable)?,
                Agent::OpenCode.render(&executable)?,
            ]
        };
        if let Some(binary) = &downloaded {
            plan.targets[0].stage(&fs::read(binary)?)?;
        }
        for target in plan.targets.iter_mut().skip(1) {
            let index = match target.agent.unwrap() {
                Agent::Pi => 0,
                Agent::OpenCode => 1,
            };
            target.stage(templates[index].as_bytes())?;
        }
        if !plan.changed() {
            println!("No changes needed.");
            return Ok(());
        }
        plan.commit()?;
        plan.report();
        Ok(())
    })();
    if result.as_ref().is_err_and(transaction::busy) {
        let handed_off = task.handoff(&executable, work, &plan);
        if handed_off.is_err() {
            plan.cleanup();
        }
        return handed_off;
    }
    plan.cleanup();
    result
}
