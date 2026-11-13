use super::view_component_gem::{app, KNOWN_GAPS, SCRIPT};

#[test]
fn a_component_renders_its_template_on_ruby() {
    let Some(app) = app() else { return };
    let run = app.run_ruby(SCRIPT);
    let unknown: Vec<_> = run
        .errors
        .iter()
        .filter(|e| !KNOWN_GAPS.iter().any(|gap| e.contains(gap)))
        .collect();
    assert!(unknown.is_empty(), "{unknown:#?}");
    for gap in KNOWN_GAPS {
        assert!(run.errors.iter().any(|e| e.contains(gap)), "{gap} is answered now: drop it from KNOWN_GAPS\n{:#?}", run.errors);
    }
    assert!(run.program_succeeded(), "{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("view_component passed"), "{}", run.stdout);
}


/// A polymorphic slot `define_method`s setters named as it runs, which no tree carries: reported, not left to raise.
#[test]
fn a_polymorphic_slot_is_reported() {
    let Some(app) = app() else { return };
    let (_, errors) = app
        .write(
            "app/components/badge_component.rb",
            "class BadgeComponent < ViewComponent::Base\n  renders_one :icon, types: { svg: \"SvgIcon\" }\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("polymorphic slot")), "{errors:#?}");
}

/// The preview pages of the gem's engine: their controller is Rails' `Rails::ApplicationController`'s, no route
/// dispatches to it (the engine draws the preview routes only when previews are shown), and the concern that renders
/// them runs only as its action. The Spinel tree leaves them out, as they call route helpers no route defines.
#[test]
fn the_preview_pages_no_route_reaches_are_left_out_of_the_spinel_tree() {
    let Some(app) = app() else { return };
    let (emitted, _errors) = app.emit(roundhouse::project::BuildTarget::Spinel);
    assert!(!emitted.join("app/views/view_components/index.rb").exists());
    assert!(!emitted.join("app/views/view_components/previews.rb").exists());
    assert!(emitted.join("app/views/articles/index.rb").exists());
}
