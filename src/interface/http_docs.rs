//! src/interface/http_docs.rs serves public Swagger UI and the HTTP v1 OpenAPI
//! contract. Its router contains documentation only; API authorization stays in http.rs.

use axum::{
    Json, Router,
    extract::Request,
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::{Html, Response},
    routing::get,
};
use serde_json::{Value, json};

pub(super) fn router() -> Router {
    Router::new()
        .route(
            "/api/v1/docs",
            get(|| async { Html(include_str!("swagger.html")) }),
        )
        .route(
            "/api/v1/docs/init.js",
            get(|| async {
                (
                    [(
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    )],
                    include_str!("swagger.js"),
                )
            }),
        )
        .route("/api/v1/openapi.json", get(|| async { Json(contract()) }))
        .layer(middleware::from_fn(document_headers))
}

async fn document_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; script-src 'self' https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.33.1/swagger-ui-bundle.js; style-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.33.1/swagger-ui.css; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
        ),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

fn contract() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {
            "title": "Garmin Pace HTTP API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Local Garmin snapshots over authenticated MCP. Authorize with an administrator-issued per-user key. Each request uses that user's profile; keys are checked on every call. MCP tools read local data and may write local workout drafts/proposals; the Garmin save, reconcile, delivery and device-list tools succeed only for users granted with `pace users allow-garmin-writes` and a quoted user instruction. User administration and Garmin synchronization are CLI-only."
        },
        "servers": [{"url": "/"}],
        "security": [{"bearerAuth": []}],
        "paths": {
            "/api/v1/health": {"get": {
                "operationId": "getHealth", "summary": "Check authenticated service health",
                "description": "Checks the access registry and opens the authorized profile's local storage. Does not contact Garmin or assert that snapshots are fresh.",
                "responses": responses(json!({"type":"object","required":["status"],"properties":{"status":{"type":"string","enum":["ok"]}}}))
            }},
            "/api/v1/mcp": {"post": {
                "operationId": "sendMcpRequest", "summary": "Send an MCP JSON-RPC message",
                "description": "Stateless Streamable HTTP with JSON responses. Send Accept: application/json, text/event-stream and Content-Type: application/json. Initialize to negotiate the MCP version; use tools/list for the current tool names and input schemas, then tools/call. No session ID is required. JSON-RPC and tool errors may be returned with HTTP 200; inspect error and result.isError. GET event streams and DELETE sessions are unsupported (405).",
                "parameters": [{"name":"MCP-Protocol-Version","in":"header","required":false,"schema":{"type":"string","default":"2025-11-25"},"description":"Negotiated MCP protocol version; initialization negotiates it in the JSON body."}],
                "requestBody": {"required":true,"content":{"application/json":{
                    "schema":{"$ref":"#/components/schemas/McpMessage"},
                    "examples": {
                        "initialize":{"summary":"Initialize MCP","value":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"swagger-ui","version":"1"}}}},
                        "tools":{"summary":"Discover tools and their schemas","value":{"jsonrpc":"2.0","id":2,"method":"tools/list"}},
                        "status":{"summary":"Read the authorized profile's local status","value":{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_status","arguments":{}}}}
                    }
                }}},
                "responses": mcp_responses()
            }}
        },
        "components": components()
    })
}

fn responses(schema: Value) -> Value {
    json!({
        "200":{"description":"Successful response","content":{"application/json":{"schema":schema}}},
        "400":{"description":"Invalid request or forbidden query string"},
        "401":{"description":"Missing, invalid, expired or revoked key, or disabled user","headers":{"WWW-Authenticate":{"schema":{"type":"string"}}}},
        "403":{"description":"Untrusted Host or browser Origin"},
        "408":{"description":"Configured request timeout exceeded"},
        "503":{"description":"Access registry/profile storage unavailable"}
    })
}

fn mcp_responses() -> Value {
    let mut value = responses(json!({"$ref":"#/components/schemas/McpResponse"}));
    value["202"] = json!({"description":"Notification accepted; no response body"});
    value["406"] =
        json!({"description":"Accept must include application/json and text/event-stream"});
    value["413"] = json!({"description":"Configured request body limit exceeded"});
    value["415"] = json!({"description":"Content-Type must be application/json"});
    value
}

fn components() -> Value {
    json!({
        "securitySchemes":{"bearerAuth":{"type":"http","scheme":"bearer","description":"Use the key file issued by pace users issue-key. Paste only the key, without the Bearer prefix. Keys must be active and unexpired; revoke/disable takes effect on the next request."}},
        "schemas": {
            "McpMessage":{"type":"object","required":["jsonrpc","method"],"properties":{
                "jsonrpc":{"type":"string","enum":["2.0"]},
                "id":{"oneOf":[{"type":"integer"},{"type":"string"}],"description":"Omit for notifications, which receive HTTP 202."},
                "method":{"type":"string"},"params":{"type":"object","additionalProperties":true}
            }},
            "McpResponse":{"type":"object","required":["jsonrpc","id"],"properties":{
                "jsonrpc":{"type":"string","enum":["2.0"]},
                "id":{"oneOf":[{"type":"integer"},{"type":"string"}],"nullable":true},
                "result":{"type":"object","additionalProperties":true},
                "error":{"type":"object","required":["code","message"],"properties":{"code":{"type":"integer"},"message":{"type":"string"},"data":{}}}
            }}
        }
    })
}
