use super::*;

#[test]
#[ignore = "requires a disposable empty libSQL server in AGENTSYNC_TEST_LIBSQL_URL"]
fn libsql_storage_contract() {
    let url = std::env::var("AGENTSYNC_TEST_LIBSQL_URL").expect("set AGENTSYNC_TEST_LIBSQL_URL");
    let mut db = LibsqlDatabase::connect(&url).unwrap();
    db.migrate().unwrap();
    db.migrate().unwrap();
    let author = db.author_identity().unwrap();
    assert_eq!(author, "libsql@remote");
    db.register_user(&author).unwrap();
    let context = SyncContext {
        scope: Scope::Global,
        project_root: Default::default(),
        project_key: String::new(),
    };
    let modified = UNIX_EPOCH + Duration::from_micros(1_234_567_890);
    let resource = LocalResource {
        kind: ResourceKind::Skills,
        path: "skills/contract/SKILL.md".into(),
        content: b"binary\x00\xff".to_vec(),
        source: Default::default(),
        modified_at: modified,
    };
    let history = LocalResource {
        kind: ResourceKind::Histories,
        path: "sessions/contract.jsonl".into(),
        content: b"private history".to_vec(),
        ..resource.clone()
    };
    assert_eq!(
        db.push_with_visibility(
            &author,
            AgentName::Codex,
            &context,
            &[
                (&resource, Visibility::Public),
                (&history, Visibility::Public)
            ]
        )
        .unwrap(),
        2
    );
    let pulled = db
        .pull(
            &author,
            &author,
            AgentName::Codex,
            &context,
            Some(ResourceKind::Skills),
        )
        .unwrap();
    assert_eq!(pulled[0].content, resource.content);
    assert_eq!(pulled[0].modified_at, Some(modified));
    assert_eq!(pulled[0].sync_version, 1);
    db.push_with_visibility(
        &author,
        AgentName::Codex,
        &context,
        &[(&resource, Visibility::Public)],
    )
    .unwrap();
    assert_eq!(
        db.pull(
            &author,
            &author,
            AgentName::Codex,
            &context,
            Some(ResourceKind::Skills)
        )
        .unwrap()[0]
            .sync_version,
        2
    );
    let public = db
        .pull(
            "other@example.com",
            &author,
            AgentName::Codex,
            &context,
            None,
        )
        .unwrap();
    assert_eq!(public.len(), 1);
    assert_eq!(public[0].kind, ResourceKind::Skills);
    let private = db
        .pull(
            &author,
            &author,
            AgentName::Codex,
            &context,
            Some(ResourceKind::Histories),
        )
        .unwrap();
    assert_eq!(private[0].visibility, Visibility::Private);
    assert_eq!(
        db.skills("other@example.com", AgentName::Codex)
            .unwrap()
            .len(),
        1
    );
    let listed = db
        .list(ListFilter {
            viewer_email: &author,
            agent: AgentName::Codex,
            scope: Some(Scope::Global),
            kind: Some(ResourceKind::Skills),
            project_key: None,
            visibility: Some(Visibility::Public),
            author: Some(&author),
        })
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].size, resource.content.len() as i64);
    assert_eq!(listed[0].sync_version, 2);
    assert!(
        db.run(async {
            db.connection
                .execute(
                    "UPDATE agentsync_resources SET visibility='public' WHERE kind='histories'",
                    (),
                )
                .await?;
            Ok(())
        })
        .is_err()
    );
    // Cause a server-side error after the first row and prove the batch rolls back.
    db.run(async {db.connection.execute("CREATE TRIGGER reject_contract_insert BEFORE INSERT ON agentsync_resources WHEN NEW.path='skills/contract/fail' BEGIN SELECT RAISE(ABORT,'test failure'); END",()).await?; Ok(())}).unwrap();
    let good = LocalResource {
        path: "skills/contract/rollback".into(),
        ..resource.clone()
    };
    let bad = LocalResource {
        path: "skills/contract/fail".into(),
        ..resource.clone()
    };
    assert!(
        db.push_with_visibility(
            &author,
            AgentName::Codex,
            &context,
            &[(&good, Visibility::Private), (&bad, Visibility::Private)]
        )
        .is_err()
    );
    let after = db
        .pull(
            &author,
            &author,
            AgentName::Codex,
            &context,
            Some(ResourceKind::Skills),
        )
        .unwrap();
    assert_eq!(after.len(), 1);
    db.run(async {
        db.connection
            .execute("DROP TRIGGER reject_contract_insert", ())
            .await?;
        Ok(())
    })
    .unwrap();
    db.run(async {
        db.connection
            .execute("UPDATE agentsync_schema SET version=999", ())
            .await?;
        Ok(())
    })
    .unwrap();
    assert!(
        db.migrate()
            .unwrap_err()
            .to_string()
            .contains("unsupported")
    );
    db.run(async {
        db.connection
            .execute("UPDATE agentsync_schema SET version=1", ())
            .await?;
        Ok(())
    })
    .unwrap();
    assert!(db.ping().is_ok());
}
