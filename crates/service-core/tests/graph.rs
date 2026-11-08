use rustos_service_core::{
    config,
    graph::{Catalog, ErrorKind},
};
fn service(name: &str, edges: &str) -> String {
    format!("Version=1\nKind=service\nName={name}\nExec=/BIN/GLOW.SVC\n{edges}")
}
fn load(files: &[(&str, &str)]) -> Result<Catalog, rustos_service_core::graph::Error> {
    Catalog::load(
        &files
            .iter()
            .map(|(name, text)| (*name, text.as_bytes()))
            .collect::<Vec<_>>(),
    )
}
fn names(catalog: &Catalog, order: &[usize]) -> Vec<String> {
    order
        .iter()
        .map(|&id| catalog.unit(id).unwrap().name.clone())
        .collect()
}
#[test]
fn activation_closure_distinguishes_ordering_and_optional_membership() {
    let a = service("a", "After=unused\n");
    let b = service("b", "Requires=a\nAfter=optional\n");
    let optional = service("optional", "");
    let unused = service("unused", "");
    let boot = "Version=1\nKind=target\nName=boot\nRequires=b\nWants=optional\n";
    let catalog = load(&[
        ("B.UNIT", &b),
        ("BOOT.UNIT", boot),
        ("A.UNIT", &a),
        ("OPT.UNIT", &optional),
        ("UNUSED.UNIT", &unused),
    ])
    .unwrap();
    let plan = catalog.plan_start("boot").unwrap();
    assert_eq!(names(&catalog, &plan.order), ["a", "optional", "b", "boot"]);
    let unused_id = catalog.lookup("unused").unwrap();
    assert_eq!(
        plan.selected & (1 << unused_id),
        0,
        "After must not pull a unit in"
    );
    assert_eq!(plan.selected.count_ones(), 4);
}
#[test]
fn catalog_order_is_independent_of_filesystem_enumeration() {
    let a = service("a", "");
    let z = service("z", "Requires=a\nAfter=a\n");
    let x = load(&[("Z.UNIT", &z), ("A.UNIT", &a)]).unwrap();
    let y = load(&[("A.UNIT", &a), ("Z.UNIT", &z)]).unwrap();
    assert_eq!(names(&x, &x.plan_start("z").unwrap().order), ["a", "z"]);
    assert_eq!(names(&y, &y.plan_start("z").unwrap().order), ["a", "z"]);
}
#[test]
fn unknown_units_and_duplicate_sources_fail_before_activation() {
    let a = service("a", "Requires=missing\n");
    assert_eq!(
        load(&[("A.UNIT", &a)]).unwrap_err().kind,
        ErrorKind::Missing("missing".into())
    );
    let a = service("a", "");
    assert_eq!(
        load(&[("A.UNIT", &a), ("a.unit", &a)]).unwrap_err().kind,
        ErrorKind::DuplicateSource
    );
    assert_eq!(
        load(&[("LONGNAME.UNIT", &a), ("ALIAS.UNIT", &a)])
            .unwrap_err()
            .kind,
        ErrorKind::DuplicateName
    );
    assert_eq!(
        load(&[("../A.UNIT", &a)]).unwrap_err().kind,
        ErrorKind::InvalidSource
    );
    let c = load(&[("A.UNIT", &a)]).unwrap();
    assert_eq!(
        c.plan_start("missing").unwrap_err().kind,
        ErrorKind::UnknownTarget
    );
}
#[test]
fn cycles_report_the_actual_closed_path_including_mixed_edges() {
    let a = service("a", "Requires=b\n");
    let b = service("b", "After=c\n");
    let c = service("c", "Requires=a\n");
    assert_eq!(
        load(&[("A.UNIT", &a), ("B.UNIT", &b), ("C.UNIT", &c)])
            .unwrap_err()
            .kind,
        ErrorKind::Cycle(vec!["a".into(), "b".into(), "c".into(), "a".into()])
    );
    for edge in ["Requires=a\n", "After=a\n", "Wants=a\n"] {
        let a = service("a", edge);
        assert_eq!(
            load(&[("A.UNIT", &a)]).unwrap_err().kind,
            ErrorKind::Cycle(vec!["a".into(), "a".into()])
        );
    }
}
#[test]
fn membership_is_not_an_ordering_barrier() {
    let a = service("a", "Wants=b\n");
    let b = service("b", "Wants=a\n");
    let catalog = load(&[("A.UNIT", &a), ("B.UNIT", &b)]).unwrap();
    assert_eq!(
        names(&catalog, &catalog.plan_start("a").unwrap().order),
        ["a", "b"]
    );
}
#[test]
fn validates_unselected_cycles_and_preserves_source_parse_location() {
    let a = service("a", "");
    let unused = service("unused", "After=unused\n");
    assert!(matches!(
        load(&[("A.UNIT", &a), ("OTHER.UNIT", &unused)])
            .unwrap_err()
            .kind,
        ErrorKind::Cycle(_)
    ));
    let error = load(&[("A.UNIT", &a), ("BAD.UNIT", "Bogus=1\n")]).unwrap_err();
    assert_eq!(error.source, 1);
    assert!(matches!(
        error.kind,
        ErrorKind::Parse(config::Error {
            line: 1,
            kind: config::ErrorKind::UnknownKey
        })
    ));
}
#[test]
fn supports_all_64_catalog_bits_and_rejects_the_65th_unit() {
    let files: Vec<_> = (0..64)
        .map(|n| (format!("U{n}.UNIT"), service(&format!("u{n:02}"), "")))
        .collect();
    let sources: Vec<_> = files
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_bytes()))
        .collect();
    let catalog = Catalog::load(&sources).unwrap();
    assert_eq!(catalog.len(), 64);
    assert_eq!(catalog.plan_start("u63").unwrap().selected, 1u64 << 63);
    let mut overflow = sources;
    overflow.push(("OVER.UNIT", b""));
    assert_eq!(Catalog::load(&overflow).unwrap_err().kind, ErrorKind::Limit);
}
#[test]
fn aggregate_byte_and_edge_budgets_are_enforced() {
    let mut data = service("a", "");
    data.extend(std::iter::repeat(' ').take(config::MAX_FILE_BYTES - data.len()));
    let files: Vec<_> = (0..17)
        .map(|n| {
            (
                format!("U{n}.UNIT"),
                data.replace("Name=a", &format!("Name=u{n}")),
            )
        })
        .collect();
    let sources: Vec<_> = files
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_bytes()))
        .collect();
    assert_eq!(Catalog::load(&sources).unwrap_err().kind, ErrorKind::Limit);
}

#[test]
fn aggregate_edges_fail_before_resolving_or_activating_any_node() {
    let files: Vec<_> = (0..32)
        .map(|n| {
            let edges: String = ((n + 1)..32)
                .map(|next| format!("Requires=u{next:02}\n"))
                .collect();
            (format!("U{n}.UNIT"), service(&format!("u{n:02}"), &edges))
        })
        .collect();
    let sources: Vec<_> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_bytes()))
        .collect();
    assert_eq!(Catalog::load(&sources).unwrap_err().kind, ErrorKind::Limit);
}
