use prost_reflect::{Kind, MessageDescriptor};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn document_for(mode: crate::projection::ResponseMode) -> Value {
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
        let parameters: Vec<_> = crate::query_params::fields(
            moenotes_proto::pool().get_message_by_name(method.input).unwrap()
        ).into_iter().map(|(field_name, field)| {
            let mut value = kind(field.kind(), &mut schemas);
            if field.is_list() {
                value = json!({"type":"array","items":value,"maxItems":100});
                if crate::query_params::required(name, &field_name) {
                    value["minItems"] = json!(1);
                }
            }
            json!({"name":field_name,"in":"query","required":crate::query_params::required(name, &field_name),
                "style":"form","explode":true,"deprecated":field.options().get_field_by_name("deprecated").is_some_and(|v|v.as_bool()==Some(true)),
                "schema":value,"description":if field.is_list() {"Repeat the parameter for each item; order and duplicates are preserved."} else {"Single occurrence only. Nested filters use dotted field names."}})
        }).collect();
        paths.insert(path, json!({"get":{
            "operationId":name,"tags":["read queries"],
            "description":"No request body. Public mode removes unapproved/account-specific fields; raw mode is operator-only. Query string limited to 8192 bytes and 256 parameters.",
            "security":[{"apiKey":[]}],
            "parameters":parameters,
            "responses":{
                "200":{"description":"Raw protobuf JSON; 64-bit integers are strings. Fetch time is Unix milliseconds.",
                    "headers":{"X-Moenotes-Cache":{"schema":{"type":"string","enum":["HIT","MISS","COALESCED"]}},"X-Moenotes-Fetched-At":{"schema":{"type":"string"}}},
                    "content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{}",method.output)}}}},
                "400":{"description":"Invalid query"},"401":{"description":"Missing or invalid HTTP API key"},
                "403":{"description":"Route prohibited by response policy"},"404":{"description":"Route disabled or unknown"},"405":{"description":"Only GET is supported; HEAD does not query upstream"},"429":{"description":"Local request queue full"},
                "502":{"description":"Upstream business, transport or protocol error"},
                "503":{"description":"Upstream session or availability blocked"},"504":{"description":"Request deadline exceeded"}
            }
        }}));
    }
    if mode == crate::projection::ResponseMode::Public {
        paths.remove("/v1/circles/recommended");
        for (name, field) in [
            ("app.event.GetChallengeMusicRankingResponse", "myRank"),
            ("app.event.GetChallengeMusicRankingResponse", "myScore"),
            ("app.livemusic.GetRankingResponse", "myRank"),
            (
                "app.player.GetPlayerFavoriteStatusResponse",
                "isSentFavorite",
            ),
        ] {
            if let Some(properties) = schemas
                .get_mut(name)
                .and_then(|v| v.get_mut("properties"))
                .and_then(Value::as_object_mut)
            {
                properties.remove(field);
            }
        }
    }
    for path in ["/v1/status", "/readyz"] {
        paths.insert(path,json!({"get":{"security":[{"apiKey":[]}],"responses":{"200":{"description":"Sanitized local operational state"},"401":{"description":"Missing API key"},"503":{"description":"Not ready"}}}}));
    }
    json!({"openapi":"3.1.0","info":{"title":"moenotes-api","version":env!("CARGO_PKG_VERSION"),"description":"Experimental GET query gateway with limited live validation. Not an official or stable API."},
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
        Kind::Enum(enumeration) => {
            json!({"oneOf":[{"type":"string","enum":enumeration.values().map(|v|v.name().to_owned()).collect::<Vec<_>>()},{"type":"integer","format":"int32"}]})
        }
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
