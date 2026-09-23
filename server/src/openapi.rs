use prost_reflect::{Kind, MessageDescriptor};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn document() -> Value {
    let mut paths = BTreeMap::new();
    let mut schemas = BTreeMap::new();
    for &(path, name) in crate::ROUTES {
        let method = moenotes_client::METHODS
            .iter()
            .find(|m| m.name == name)
            .unwrap();
        for message in [method.input, method.output] {
            schema(
                moenotes_proto::pool().get_message_by_name(message).unwrap(),
                &mut schemas,
            );
        }
        paths.insert(path, json!({"post":{
            "operationId":name,"tags":["experimental raw queries"],
            "description":"Disabled unless enable_experimental_raw=true. Raw response may include operator-account-specific fields. No online compatibility guarantee.",
            "security":[{"apiKey":[]}],
            "requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{}",method.input)}}}},
            "responses":{
                "200":{"description":"Raw protobuf JSON; 64-bit integers are strings. Fetch time is Unix milliseconds.",
                    "headers":{"X-Moenotes-Cache":{"schema":{"type":"string","enum":["HIT","MISS","COALESCED"]}},"X-Moenotes-Fetched-At":{"schema":{"type":"string"}}},
                    "content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{}",method.output)}}}},
                "400":{"description":"Invalid query"},"401":{"description":"Missing or invalid HTTP API key"},
                "404":{"description":"Experimental route disabled"},"429":{"description":"Local request queue full"},
                "502":{"description":"Upstream business, transport or protocol error"},
                "503":{"description":"Upstream session or availability blocked"},"504":{"description":"Request deadline exceeded"}
            }
        }}));
    }
    json!({"openapi":"3.1.0","info":{"title":"moenotes-api","version":"0.1.0-dev","description":"Experimental offline-validated query gateway. Not an official or stable API."},
        "paths":paths,"components":{"securitySchemes":{"apiKey":{"type":"http","scheme":"bearer"}},"schemas":schemas}})
}

fn schema(message: MessageDescriptor, schemas: &mut BTreeMap<String, Value>) {
    let name = message.full_name().to_owned();
    if schemas.contains_key(&name) {
        return;
    }
    schemas.insert(name.clone(), json!({}));
    let mut properties = BTreeMap::new();
    for field in message.fields() {
        let value = if field.is_map() {
            let Kind::Message(entry) = field.kind() else {
                unreachable!()
            };
            json!({"type":"object","additionalProperties":kind(entry.get_field_by_name("value").unwrap().kind(),schemas)})
        } else if field.is_list() {
            json!({"type":"array","items":kind(field.kind(),schemas)})
        } else {
            kind(field.kind(), schemas)
        };
        properties.insert(field.json_name().to_owned(), value);
    }
    schemas.insert(
        name,
        json!({"type":"object","properties":properties,"additionalProperties":false}),
    );
}
fn kind(kind: Kind, schemas: &mut BTreeMap<String, Value>) -> Value {
    match kind {
        Kind::Message(message) => {
            let name = message.full_name().to_owned();
            schema(message, schemas);
            json!({"$ref":format!("#/components/schemas/{name}")})
        }
        Kind::Enum(_) => json!({"oneOf":[{"type":"string"},{"type":"integer","format":"int32"}]}),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
            json!({"type":"string","pattern":"^-?[0-9]+$"})
        }
        Kind::Uint64 | Kind::Fixed64 => json!({"type":"string","pattern":"^[0-9]+$"}),
        Kind::Bool => json!({"type":"boolean"}),
        Kind::String => json!({"type":"string"}),
        Kind::Bytes => json!({"type":"string","contentEncoding":"base64"}),
        Kind::Float | Kind::Double => {
            json!({"oneOf":[{"type":"number"},{"type":"string","enum":["NaN","Infinity","-Infinity"]}]})
        }
        _ => json!({"type":"integer"}),
    }
}
