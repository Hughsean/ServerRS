use server_rs::app::agent::graph::{EffectId, NodeId, RunId, RunStep};

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
