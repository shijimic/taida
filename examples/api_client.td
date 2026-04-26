// api_client.td -- JSON API Client Example (Molten Iron)
//
// Demonstrates: JSON schema casting (JSON[raw, Schema]()),
// ]=> unmolding, error handling, and pipeline operators.


// ── JSON Molten Iron Pattern ──────────────────────────────
//
// External data (JSON) enters Taida through a schema (TypeDef).
// The JSON type is opaque — you must define a schema to extract values.

// Define schemas for the API response
Pilot = @(id: Int, name: Str, active: Bool)
ApiResponse = @(pilots: @[Pilot], total: Int)

// Simulated API response (in real usage, this comes from http)
rawResponse <= '{"pilots":[{"id":1,"name":"Asuka","active":true},{"id":2,"name":"Rei","active":false},{"id":3,"name":"Shinji","active":true}],"total":3}'

// ── Error ceiling for the whole program ──────────────────

|== parseError: Error =
  stdout("Error: " + parseError.message)
  ><
=> :Int

// ── Step 1: Cast JSON through schema ─────────────────────

JSON[rawResponse, ApiResponse]() ]=> data

stdout("=== API Client Demo ===")
stdout("")

// ── Step 2: Access typed values ──────────────────────────

totalCount <= data.total
stdout("Total pilots: " + totalCount.toString())

// ── Step 3: Work with typed pilot list ───────────────────

pilots <= data.pilots
stdout("Pilots from API:")
stdout("")

// Helper: convert active flag to status label
activeLabel b =
  | b |> "active"
  | _ |> "inactive"
=> :Str

// Process each pilot (already typed)
printPilot pilot =
  statusLabel <= activeLabel(pilot.active)
  stdout("  #" + pilot.id.toString() + " " + pilot.name + " (" + statusLabel + ")")
  pilot.active
=> :Bool

Map[pilots, printPilot]() ]=> activeFlags

stdout("")

// ── Step 4: Use list methods on processed data ───────────

identity x =
  x
=> :Bool

Count[activeFlags, identity]() ]=> activeCount
stdout("Active pilots: " + activeCount.toString())

hasAnyActive <= activeFlags.any(identity)
stdout("Has active pilots: " + hasAnyActive.toString())

allActive <= activeFlags.all(identity)
stdout("All active: " + allActive.toString())

// ── Step 5: Build a new JSON payload ─────────────────────

stdout("")
stdout("=== Building Request Payload ===")

requestData <= @(
  action <= "update",
  targets <= @[1, 3],
  metadata <= @(
    updated_by <= "admin",
    reason <= "batch activation"
  )
)

// Convert Taida value to JSON string (output direction — still works)
payload <= jsonEncode(requestData)
stdout("Request payload:")
stdout("  " + payload)

// ── Step 6: JSON round-trip with schema ──────────────────

stdout("")
stdout("=== JSON Round-Trip ===")

// Parse the payload back with a schema
RequestData = @(action: Str, metadata: @(updatedBy: Str, reason: Str))
JSON[payload, RequestData]() ]=> parsed

stdout("Action: " + parsed.action)
stdout("Reason: " + parsed.metadata.reason)

stdout("")
stdout("=== Done ===")
