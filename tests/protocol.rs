use async_trait::async_trait;
use campus_mcp::{backend::Backend, server::CampusServer};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct MockBackend(AtomicUsize);
#[async_trait]
impl Backend for MockBackend {
    async fn call(&self, name: &str, args: Value) -> Result<CallToolResult, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if name == "mail_read" {
            return Err("Fixture authentication required".into());
        }
        Ok(CallToolResult::structured(json!({"tool":name,"args":args})))
    }
}
fn request(name: &str, args: Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name.to_owned()).with_arguments(args.as_object().unwrap().clone())
}
#[tokio::test]
async fn negotiated_tools_dispatch_validation_and_errors() {
    let backend = Arc::new(MockBackend::default());
    let (server_io, client_io) = tokio::io::duplex(65536);
    let server = CampusServer::new(backend.clone(), true, true);
    let task = tokio::spawn(async move {
        server
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 10);
    let status = client
        .call_tool(request("campus_status", json!({})))
        .await
        .unwrap();
    assert_eq!(status.structured_content.unwrap()["read_only"], true);
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);
    let result = client
        .call_tool(request("lms_notices", json!({"limit":3})))
        .await
        .unwrap();
    assert_eq!(
        result.structured_content.unwrap(),
        json!({"tool":"lms_notices","args":{"limit":3}})
    );
    for args in [
        json!({"limit":"3"}),
        json!({"limit":-1}),
        json!({"unknown":true}),
    ] {
        let response = client.call_tool(request("lms_notices", args)).await;
        assert!(response.is_err() || response.unwrap().is_error == Some(true));
    }
    for (name, args) in [
        ("lms_courses", json!({"limit":1})),
        ("lms_assignments", json!({"all_terms":true})),
    ] {
        let response = client.call_tool(request(name, args)).await;
        assert!(response.is_err() || response.unwrap().is_error == Some(true));
    }
    assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    let failure = client
        .call_tool(request("mail_read", json!({"id":"fixture"})))
        .await
        .unwrap();
    assert_eq!(failure.is_error, Some(true));
    assert_eq!(
        failure.structured_content.unwrap()["error"],
        "Fixture authentication required"
    );
    assert!(
        client
            .call_tool(request("not_a_tool", json!({})))
            .await
            .is_err()
    );
    client.cancel().await.unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn disabled_modules_are_neither_advertised_nor_callable() {
    for (lms, mail, count) in [(true, false, 8), (false, true, 4)] {
        let backend = Arc::new(MockBackend::default());
        let (server_io, client_io) = tokio::io::duplex(65536);
        let server = CampusServer::new(backend.clone(), lms, mail);
        let task = tokio::spawn(async move {
            server
                .serve(server_io)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ().serve(client_io).await.unwrap();
        let tools = client.list_all_tools().await.unwrap();
        assert_eq!(tools.len(), count);
        let disabled = if lms { "mail_list" } else { "lms_courses" };
        assert!(!tools.iter().any(|t| t.name == disabled));
        assert!(
            client
                .call_tool(request(disabled, json!({})))
                .await
                .is_err()
        );
        assert_eq!(backend.0.load(Ordering::SeqCst), 0);
        client.cancel().await.unwrap();
        task.await.unwrap();
    }
}
