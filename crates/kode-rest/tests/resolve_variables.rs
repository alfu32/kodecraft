use kode_rest::{
    model::{Environment, NodePath},
    resolve::VariableResolver,
};
use serde_json::json;
#[test]
fn hierarchy_environment_precedence_disabled_values_and_literal_replacements_match_source() {
    let collection = json!({"variable":[{"key":"host","value":"collection"},{"key":"base","value":"https://{{host}}"},{"key":"retained","value":"parent"}],"item":[{"name":"Folder","variable":[{"key":"host","value":"folder"}],"item":[{"name":"Call","variable":[{"key":"host","value":"request"},{"key":"retained","value":"disabled","disabled":true}]}]}]});
    let environment:Environment=serde_json::from_value(json!({"id":"fixture","name":"env","values":[{"key":"host","value":"environment","secret":true},{"key":"ignored","value":"inactive","enabled":false}]})).unwrap();
    let resolver =
        VariableResolver::new(&collection, &NodePath(vec![0, 0]), Some(&environment)).unwrap();
    assert_eq!(
        resolver
            .resolve("{{base}}/{{retained}}/$literal\\path")
            .unwrap()
            .value,
        "https://environment/parent/$literal\\path"
    );
    let values: Vec<_> = resolver.effective_variables().collect();
    assert_eq!(values[0].source, "Environment");
    assert!(values[0].secret);
    assert!(!format!("{:?}", values[0]).contains("environment"));
    assert!(
        !resolver
            .effective_variables()
            .any(|value| value.key == "ignored")
    );
    assert!(VariableResolver::new(&collection, &NodePath(vec![99, 0]), None).is_err());
}
#[test]
fn cycles_and_unresolved_names_are_reported_without_recursing_forever_or_executing_text() {
    let collection = json!({"variable":[{"key":"a","value":"{{b}}"},{"key":"b","value":"{{a}}"},{"key":"object","value":{"nested":42}}]});
    let resolver = VariableResolver::new(&collection, &NodePath::default(), None).unwrap();
    let result = resolver.resolve("{{a}} {{ missing }} {{object}}").unwrap();
    assert!(result.cycles.contains("a"));
    assert!(result.unresolved.contains("missing"));
    assert_eq!(result.value, "{{a}} {{missing}} {\"nested\":42}");
}
#[test]
fn variable_depth_substitution_and_output_limits_are_visible_errors() {
    let variables: Vec<_> = (0..70)
        .map(|index| json!({"key":format!("v{index}"),"value":format!("{{{{v{}}}}}",index+1)}))
        .collect();
    let collection = json!({"variable":variables});
    let resolver = VariableResolver::new(&collection, &NodePath::default(), None).unwrap();
    assert!(resolver.resolve("{{v0}}").is_err());
    let mut variables = vec![json!({"key":"v0","value":"x"})];
    for index in 1..25 {
        variables.push(json!({"key":format!("v{index}"),"value":format!("{{{{v{}}}}}{{{{v{}}}}}",index-1,index-1)}));
    }
    let collection = json!({"variable":variables});
    let resolver = VariableResolver::new(&collection, &NodePath::default(), None).unwrap();
    assert!(resolver.resolve("{{v24}}").is_err());
    assert!(resolver.resolve(&"x".repeat(4 * 1024 * 1024 + 1)).is_err());
}
