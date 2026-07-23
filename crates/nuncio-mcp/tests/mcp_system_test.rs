//! Integration system test suite for nuncio-mcp JSON-RPC protocol server.

use nuncio_mcp::McpServer;

#[tokio::test]
async fn system_test_mcp_protocol_initialize_tools_and_resources() {
    let server = McpServer::ephemeral().await.expect("mcp server init");

    // 1. Initialize
    let init_req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let resp = server
        .handle_request_json(init_req)
        .await
        .expect("init response");
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["serverInfo"]["name"], "nuncio-mcp");

    // 2. Tools List
    let tools_req = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let resp = server
        .handle_request_json(tools_req)
        .await
        .expect("tools response");
    let tools = resp["result"]["tools"].as_array().expect("array");
    assert!(tools.iter().any(|t| t["name"] == "nuncio_mail_list"));
    assert!(tools.iter().any(|t| t["name"] == "nuncio_mail_send"));
    assert!(tools.iter().any(|t| t["name"] == "nuncio_mail_search"));
    assert!(tools.iter().any(|t| t["name"] == "nuncio_cal_list_events"));
    assert!(tools.iter().any(|t| t["name"] == "nuncio_cal_create_event"));
    assert!(tools.iter().any(|t| t["name"] == "nuncio_update_check"));
    assert!(tools.iter().any(|t| t["name"] == "nuncio_update_apply"));

    // 2b. Test Update Check & Apply Tools
    let check_req = r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"nuncio_update_check","arguments":{}}}"#;
    let resp = server.handle_request_json(check_req).await.expect("update check response");
    assert_eq!(resp["id"], 21);
    assert!(resp["result"]["content"][0]["text"].as_str().unwrap().contains("update_available"));

    let apply_req = r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"nuncio_update_apply","arguments":{"version":"0.2.0"}}}"#;
    let resp = server.handle_request_json(apply_req).await.expect("update apply response");
    assert_eq!(resp["id"], 22);
    assert!(resp["result"]["content"][0]["text"].as_str().unwrap().contains("update_initiated"));

    // 3. Tools Call (Create & List Calendar Event)
    let create_req = r#"{
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "nuncio_cal_create_event",
            "arguments": {
                "calendar_id": "work",
                "summary": "Agent Sprint Sync",
                "start_time": 1700000000,
                "end_time": 1700003600
            }
        }
    }"#;
    let resp = server
        .handle_request_json(create_req)
        .await
        .expect("create event response");
    assert_eq!(resp["id"], 3);
    assert!(resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("created"));

    // 4. Resources List & Read
    let res_req = r#"{"jsonrpc":"2.0","id":4,"method":"resources/list","params":{}}"#;
    let resp = server
        .handle_request_json(res_req)
        .await
        .expect("resources list response");
    let resources = resp["result"]["resources"]
        .as_array()
        .expect("resources array");
    assert!(resources.iter().any(|r| r["uri"] == "nuncio://mail/inbox"));

    let read_req = r#"{
        "jsonrpc": "2.0",
        "id": 5,
        "method": "resources/read",
        "params": {
            "uri": "nuncio://mail/inbox"
        }
    }"#;
    let resp = server
        .handle_request_json(read_req)
        .await
        .expect("resources read response");
    assert_eq!(resp["id"], 5);
    assert_eq!(resp["result"]["contents"][0]["uri"], "nuncio://mail/inbox");
}
