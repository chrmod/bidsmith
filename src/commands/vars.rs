use crate::schema::InputBindings;

pub fn collect(cli_vars: &[String]) -> Result<InputBindings, String> {
    let mut bindings = InputBindings::new();
    bindings.merge_env();
    for raw in cli_vars {
        let Some((name, value)) = raw.split_once('=') else {
            return Err(format!(
                "--var must be NAME=VALUE, got '{raw}' (missing '=')"
            ));
        };
        if name.is_empty() {
            return Err(format!("--var with empty name: '{raw}'"));
        }
        bindings.vars.insert(name.to_string(), value.to_string());
    }
    Ok(bindings)
}
