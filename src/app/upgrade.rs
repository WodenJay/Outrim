mod background;
mod release;
pub use background::apply;
mod transaction;

use outrim::install::{Agent, Templates};
use std::{fs, io, path::Path, process::Command};
use transaction::{Plan, Target};

pub fn render(executable: &Path) -> io::Result<()> {
    let templates = Templates::render(executable)?;
    println!(
        "{}",
        serde_json::to_string(&templates).map_err(io::Error::other)?
    );
    Ok(())
}

pub fn run(home: &Path) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let Some(task) = background::Task::acquire(&executable)? else {
        return Ok(());
    };
    let mut plan = Plan::default();
    plan.targets
        .push(Target::inspect(executable.clone(), None)?);
    for agent in Agent::ALL {
        let path = agent.path(&super::configured_agent_dir(agent, home));
        if agent.read(&path)?.is_some() {
            plan.targets.push(Target::inspect(path, Some(agent))?);
        }
    }
    let work = task.workspace()?;
    let mut prepared = false;
    let result = (|| {
        let downloaded = release::prepare(work.path(), &executable)?;
        let templates: Templates = if let Some(binary) = &downloaded {
            let output = Command::new(binary)
                .arg("__upgrade-render")
                .arg(&executable)
                .output()?;
            if !output.status.success() {
                return Err(io::Error::other("Cannot prepare integrations"));
            }
            serde_json::from_slice(&output.stdout).map_err(io::Error::other)?
        } else {
            Templates::render(&executable)?
        };
        if let Some(binary) = &downloaded {
            plan.targets[0].stage(&fs::read(binary)?)?;
        }
        for target in plan.targets.iter_mut().skip(1) {
            target.stage(templates.get(target.agent.unwrap()).as_bytes())?;
        }
        if !plan.changed() {
            println!("Already up to date.");
            return Ok(());
        }
        prepared = true;
        plan.commit()?;
        plan.report();
        Ok(())
    })();
    if prepared && result.as_ref().is_err_and(transaction::busy) {
        let handed_off = task.handoff(&executable, work, &plan);
        if handed_off.is_err() {
            plan.cleanup();
        }
        return handed_off;
    }
    plan.cleanup();
    result
}
