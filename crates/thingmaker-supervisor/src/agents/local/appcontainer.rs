//! Private mode on Windows: a local agent runs in an AppContainer with no
//! network capabilities (see the `thingmaker-appcontainer` crate and
//! `docs/plans/local-models.md` §5.4).

use std::path::{Path, PathBuf};

/// ThingMaker's own executable, as the launcher that starts `executable
/// prefix_args…` inside the container. Refuses when the container may not
/// yet reach Ollama: that needs the person's one-time permission.
pub fn wrap(executable: &Path, prefix_args: &[String]) -> Result<(PathBuf, Vec<String>), String> {
    let sid = thingmaker_appcontainer::ensure_profile()?;
    if !thingmaker_appcontainer::loopback_allowed(&sid) {
        return Err("Local agents need your permission to reach Ollama on this PC. Allow it in Providers, under Local Models.".into());
    }
    let launcher = std::env::current_exe().map_err(|error| format!("could not find ThingMaker's own program: {error}"))?;
    let mut args = vec![thingmaker_appcontainer::LAUNCH_ARG.to_string(), executable.to_string_lossy().into_owned()];
    args.extend(prefix_args.iter().cloned());
    Ok((launcher, args))
}

/// Lets the container use what a local agent needs: the workspace and its
/// own folder (read and write), and the programs it runs (read).
pub fn prepare(root: &Path, home: &Path, programs: &[PathBuf]) -> Result<(), String> {
    let sid = thingmaker_appcontainer::ensure_profile()?;
    std::fs::create_dir_all(home.join("tmp")).map_err(|error| error.to_string())?;
    thingmaker_appcontainer::grant(root, &sid, true)?;
    thingmaker_appcontainer::grant(home, &sid, true)?;
    for program in programs {
        thingmaker_appcontainer::grant(program, &sid, false)?;
    }
    Ok(())
}
