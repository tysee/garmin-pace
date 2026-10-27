/* src/interface/swagger.js initializes Swagger UI for this server's HTTP contract.
 * It keeps authorization in memory and restricts interactive requests to the local API. */
"use strict";

if (typeof SwaggerUIBundle === "function") {
  SwaggerUIBundle({
    url: "/api/v1/openapi.json",
    dom_id: "#swagger-ui",
    presets: [SwaggerUIBundle.presets.apis],
    layout: "BaseLayout",
    validatorUrl: null,
    persistAuthorization: false,
    queryConfigEnabled: false,
    supportedSubmitMethods: ["get", "post"],
    requestInterceptor(request) {
      const target = new URL(request.url, window.location.origin);
      if (target.origin !== window.location.origin || target.search ||
          !["/api/v1/openapi.json", "/api/v1/health", "/api/v1/mcp"].includes(target.pathname)) {
        throw new Error("Swagger requests must use this server's API without query parameters.");
      }
      if (target.pathname === "/api/v1/mcp") {
        request.headers.Accept = "application/json, text/event-stream";
      }
      return request;
    },
    onComplete() {
      document.getElementById("swagger-status").hidden = true;
    },
  });
}
