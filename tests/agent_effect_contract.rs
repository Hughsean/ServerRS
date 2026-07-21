use server_rs::app::agent::graph::{
    AgentEffect, EffectId, NoEffect, NodeId, NodeResult, RunId, RunStep, UsageDelta,
};
use server_rs::domain::agent::AgentUpdate;

fn node(value: &str) -> NodeId {
    NodeId::try_from(value).unwrap()
}

#[test]
fn effect_id_is_stable_and_each_coordinate_is_significant() {
    let run = RunId::new();
    let other_run = RunId::new();
    let step_1 = RunStep::try_from(1).unwrap();
    let step_2 = RunStep::try_from(2).unwrap();

    let original = EffectId::new(run, step_1, node("persist"), 0);
    assert_eq!(original, EffectId::new(run, step_1, node("persist"), 0));
    assert_ne!(
        original,
        EffectId::new(other_run, step_1, node("persist"), 0)
    );
    assert_ne!(original, EffectId::new(run, step_2, node("persist"), 0));
    assert_ne!(original, EffectId::new(run, step_1, node("other"), 0));
    assert_ne!(original, EffectId::new(run, step_1, node("persist"), 1));
    assert_eq!(original.run_id(), run);
    assert_eq!(original.step(), step_1);
    assert_eq!(original.node_id(), &node("persist"));
    assert_eq!(original.ordinal(), 0);
}

struct TestEffect;

impl AgentEffect for TestEffect {
    type Update = ();
    type Receipt = ();

    fn receipt_updates(_receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
        Vec::new()
    }
}

#[test]
fn node_result_carries_an_explicit_effect_without_mutating_state() {
    let result =
        NodeResult::<(), TestEffect>::with_effect(Vec::new(), TestEffect, UsageDelta::default());

    assert!(result.updates().is_empty());
    assert_eq!(result.effects().len(), 1);
}

#[test]
fn no_effect_is_debug_without_constraining_the_business_update() {
    struct OpaqueUpdate;

    fn assert_debug<T: std::fmt::Debug>() {}

    assert_debug::<NoEffect<OpaqueUpdate>>();
}
