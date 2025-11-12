pub const DASHBOARD: &str = include_str!("dashboard.html");

pub fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut output = template.to_string();

    for (key, value) in values {
        let placeholder = format!("{{{{{}}}}}", key);
        output = output.replace(&placeholder, value);
    }

    output
}
