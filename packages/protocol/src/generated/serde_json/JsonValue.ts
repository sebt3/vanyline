// Re-export of serde_json::Value as a discriminated union.
// Mirrors the `TsJsonValue` type provided by ts-rs `serde-json-impl` feature.
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };