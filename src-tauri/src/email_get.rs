//! Account-local Email/get projection. Semantics adapted from Stalwart commit
//! e7cde9b3fac32d2a254ef782efec56f2373ec5d4, email/get.rs and message/jmap/*.
//! SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
//! SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
//! Upstream license choice: AGPL-3.0-only. Local redb adaptation: Herakles Mail.
use mail_parser::{DateTime, HeaderForm, HeaderValue};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

type Error = &'static str;
pub const FULL_BODY_BYTES: u64 = 8_000_000;
const DEFAULT: &[&str] = &[
    "id",
    "blobId",
    "threadId",
    "mailboxIds",
    "keywords",
    "size",
    "receivedAt",
    "messageId",
    "inReplyTo",
    "references",
    "sender",
    "from",
    "to",
    "cc",
    "bcc",
    "replyTo",
    "subject",
    "sentAt",
    "hasAttachment",
    "preview",
    "bodyValues",
    "textBody",
    "htmlBody",
    "attachments",
];
const PART: &[&str] = &[
    "partId",
    "blobId",
    "size",
    "name",
    "type",
    "charset",
    "disposition",
    "cid",
    "language",
    "location",
];

struct Options {
    properties: Vec<String>,
    body_properties: Vec<String>,
    text: bool,
    html: bool,
    all: bool,
    max: usize,
}
fn strings(args: &Value, key: &str) -> Result<Option<Vec<String>>, Error> {
    args.get(key)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_array()
                .ok_or("invalidArguments")?
                .iter()
                .map(|v| v.as_str().map(str::to_owned).ok_or("invalidArguments"))
                .collect()
        })
        .transpose()
}
impl Options {
    fn parse(args: &Value) -> Result<Self, Error> {
        if !args.is_object() {
            return Err("invalidArguments");
        }
        for key in args.as_object().unwrap().keys() {
            if ![
                "accountId",
                "ids",
                "properties",
                "bodyProperties",
                "fetchTextBodyValues",
                "fetchHTMLBodyValues",
                "fetchAllBodyValues",
                "maxBodyValueBytes",
            ]
            .contains(&key.as_str())
            {
                return Err("invalidArguments");
            }
        }
        if args.get("accountId").is_some_and(|v| !v.is_string()) {
            return Err("invalidArguments");
        }
        let properties = strings(args, "properties")?
            .unwrap_or_else(|| DEFAULT.iter().map(|s| (*s).into()).collect());
        for property in &properties {
            if property.starts_with("header:") {
                header_spec(property)?;
            } else if !DEFAULT.contains(&property.as_str())
                && !["headers", "bodyStructure"].contains(&property.as_str())
            {
                return Err("invalidArguments");
            }
        }
        let body_properties = strings(args, "bodyProperties")?
            .unwrap_or_else(|| PART.iter().map(|s| (*s).into()).collect());
        for property in &body_properties {
            if property.starts_with("header:") {
                header_spec(property)?;
            } else if !PART.contains(&property.as_str())
                && !["headers", "subParts"].contains(&property.as_str())
            {
                return Err("invalidArguments");
            }
        }
        let flag = |key: &str| -> Result<bool, Error> {
            args.get(key)
                .filter(|v| !v.is_null())
                .map_or(Ok(false), |v| v.as_bool().ok_or("invalidArguments"))
        };
        let max = args
            .get("maxBodyValueBytes")
            .filter(|v| !v.is_null())
            .map_or(Ok(0), |v| {
                v.as_u64()
                    .filter(|n| *n <= u32::MAX as u64)
                    .ok_or("invalidArguments")
            })? as usize;
        Ok(Self {
            properties,
            body_properties,
            text: flag("fetchTextBodyValues")?,
            html: flag("fetchHTMLBodyValues")?,
            all: flag("fetchAllBodyValues")?,
            max,
        })
    }
}
struct HeaderSpec<'a> {
    name: &'a str,
    form: &'a str,
    all: bool,
}
fn header_spec(property: &str) -> Result<HeaderSpec<'_>, Error> {
    let mut fields = property
        .strip_prefix("header:")
        .ok_or("invalidArguments")?
        .split(':');
    let name = fields.next().unwrap_or_default();
    if name.is_empty() || !name.bytes().all(|b| (33..=126).contains(&b) && b != b':') {
        return Err("invalidArguments");
    }
    let mut form = "asRaw";
    let mut all = false;
    if let Some(option) = fields.next() {
        if option == "all" {
            all = true;
        } else {
            if ![
                "asRaw",
                "asText",
                "asAddresses",
                "asGroupedAddresses",
                "asMessageIds",
                "asDate",
                "asURLs",
            ]
            .contains(&option)
            {
                return Err("invalidArguments");
            }
            form = option;
            if let Some(option) = fields.next() {
                if option != "all" {
                    return Err("invalidArguments");
                }
                all = true;
            }
        }
    }
    if fields.next().is_some() {
        return Err("invalidArguments");
    }
    Ok(HeaderSpec { name, form, all })
}
fn header_form(raw: &str, form: &str) -> Value {
    let raw = raw.replace('\0', "");
    if form == "asRaw" {
        return json!(raw.trim_end());
    }
    let parser = match form {
        "asText" => HeaderForm::Text,
        "asMessageIds" => HeaderForm::MessageIds,
        "asDate" => HeaderForm::Date,
        _ => HeaderForm::Addresses,
    };
    let parsed = parser.parse(raw.as_bytes());
    match (parsed.value(), form) {
        (HeaderValue::Text(text), "asText") => json!(text),
        (HeaderValue::TextList(list), "asText") => {
            json!(list.iter().collect::<Vec<_>>().join(", "))
        }
        (HeaderValue::TextList(list), "asMessageIds") => json!(list.iter().collect::<Vec<_>>()),
        (HeaderValue::Text(text), "asMessageIds") => json!([text]),
        (HeaderValue::DateTime(date), "asDate") => {
            json!(DateTime::from_timestamp(date.to_timestamp()).to_rfc3339())
        }
        (HeaderValue::Address(list), "asAddresses") => Value::Array(
            list.mailboxes()
                .map(|m| json!({"name":m.name(),"email":m.address().unwrap_or_default()}))
                .collect(),
        ),
        (HeaderValue::Address(list), "asGroupedAddresses") => {
            if !list.has_groups() {
                json!([{"name":null,"addresses":list.mailboxes().map(|m| json!({"name":m.name(),"email":m.address().unwrap_or_default()})).collect::<Vec<_>>()}])
            } else {
                Value::Array(list.groups().map(|(name, members)| json!({"name":name,"addresses":members.map(|m| json!({"name":m.name(),"email":m.address().unwrap_or_default()})).collect::<Vec<_>>()})).collect())
            }
        }
        (HeaderValue::Address(list), "asURLs") if !list.has_groups() => Value::Array(
            list.mailboxes()
                .filter_map(|m| m.address().filter(|s| s.contains(':')).map(|s| json!(s)))
                .collect(),
        ),
        _ => Value::Null,
    }
}
fn header(object: &Value, property: &str) -> Result<Value, Error> {
    let spec = header_spec(property)?;
    // Keep exact server-projected values for old replicas without a header list.
    let Some(headers) = object["headers"].as_array() else {
        return Ok(object.get(property).cloned().unwrap_or(Value::Null));
    };
    let mut values = headers
        .iter()
        .filter(|h| {
            h["name"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(spec.name))
        })
        .map(|h| {
            h["value"]
                .as_str()
                .map_or(Value::Null, |raw| header_form(raw, spec.form))
        })
        .collect::<Vec<_>>();
    Ok(if spec.all {
        Value::Array(values)
    } else {
        values.pop().unwrap_or(Value::Null)
    })
}
fn part(object: &Value, properties: &[String]) -> Result<Value, Error> {
    if object.is_null() {
        return Ok(Value::Null);
    }
    let mut projected = Map::new();
    for property in properties {
        let value = if property.starts_with("header:") {
            header(object, property)?
        } else {
            object.get(property).cloned().unwrap_or(Value::Null)
        };
        projected.insert(property.clone(), value);
    }
    // subParts is always included recursively, even when bodyProperties omits it.
    projected.insert(
        "subParts".into(),
        match object["subParts"].as_array() {
            Some(parts) => Value::Array(
                parts
                    .iter()
                    .map(|p| part(p, properties))
                    .collect::<Result<_, _>>()?,
            ),
            None => Value::Null,
        },
    );
    Ok(Value::Object(projected))
}
fn collect_parts<'a>(value: &'a Value, types: &mut HashMap<&'a str, &'a str>) {
    match value {
        Value::Object(map) => {
            if let (Some(id), Some(kind)) = (
                map.get("partId").and_then(Value::as_str),
                map.get("type").and_then(Value::as_str),
            ) {
                types.insert(id, kind);
            }
            for key in [
                "subParts",
                "bodyStructure",
                "textBody",
                "htmlBody",
                "attachments",
            ] {
                if let Some(child) = map.get(key) {
                    collect_parts(child, types);
                }
            }
        }
        Value::Array(parts) => {
            for p in parts {
                collect_parts(p, types);
            }
        }
        _ => {}
    }
}
/// Stalwart-compatible UTF-8 truncation with ellipsis and CR removal. HTML
/// additionally avoids cutting a tag or comment in half.
fn truncate(text: &str, max: usize, html: bool) -> (bool, String) {
    if html {
        truncate_html(text, max)
    } else {
        truncate_plain(text, max)
    }
}
// Ported from Stalwart crates/email/src/message/body.rs at the pinned commit.
fn truncate_plain(text: &str, mut max_len: usize) -> (bool, String) {
    if max_len != 0 && text.len() > max_len {
        let add_dots = max_len > 6;
        if add_dots {
            max_len -= 3;
        }
        let mut result = String::with_capacity(max_len);
        for ch in text.chars() {
            if ch != '\r' {
                if ch.len_utf8() + result.len() > max_len {
                    break;
                }
                result.push(ch);
            }
        }
        if add_dots {
            result.push_str("...");
        }
        (true, result)
    } else {
        (false, text.replace('\r', ""))
    }
}

fn truncate_html(html: &str, mut max_len: usize) -> (bool, String) {
    if max_len != 0 && html.len() > max_len {
        let add_dots = max_len > 6;
        if add_dots {
            max_len -= 3;
        }

        let mut result = String::with_capacity(max_len);
        let mut in_tag = false;
        let mut in_comment = false;
        let mut last_tag_end_pos = 0;
        let mut cr_count = 0;
        for (pos, ch) in html.char_indices() {
            let mut set_last_tag = 0;
            match ch {
                '<' if !in_tag => {
                    in_tag = true;
                    if let Some("!--") = html.get(pos + 1..pos + 4) {
                        in_comment = true;
                    }
                    set_last_tag = pos;
                }
                '>' if in_tag => {
                    if in_comment {
                        if let Some("--") = html.get(pos - 2..pos) {
                            in_comment = false;
                            in_tag = false;
                            set_last_tag = pos + 1;
                        }
                    } else {
                        in_tag = false;
                        set_last_tag = pos + 1;
                    }
                }
                '\r' => {
                    cr_count += 1;
                    continue;
                }
                _ => (),
            }
            if ch.len_utf8() + pos - cr_count > max_len {
                result.push_str(
                    &html[0..if (in_tag || set_last_tag > 0) && last_tag_end_pos > 0 {
                        last_tag_end_pos
                    } else {
                        pos
                    }]
                        .replace('\r', ""),
                );
                if add_dots {
                    result.push_str("...");
                }
                break;
            } else if set_last_tag > 0 {
                last_tag_end_pos = set_last_tag;
            }
        }
        (true, result)
    } else {
        (false, html.replace('\r', ""))
    }
}
fn body_values(email: &Value, options: &Options) -> Value {
    if !options.all && !options.text && !options.html {
        return json!({});
    }
    if email.get("bodyValues").is_some_and(Value::is_null) {
        return Value::Null;
    }
    let mut wanted = HashSet::new();
    for (field, enabled) in [("textBody", options.text), ("htmlBody", options.html)] {
        if enabled {
            if let Some(parts) = email[field].as_array() {
                wanted.extend(parts.iter().filter_map(|p| p["partId"].as_str()));
            }
        }
    }
    let mut types = HashMap::new();
    collect_parts(email, &mut types);
    let mut result = Map::new();
    if let Some(values) = email["bodyValues"].as_object() {
        for (id, value) in values {
            if !options.all && !wanted.contains(id.as_str()) {
                continue;
            }
            if types
                .get(id.as_str())
                .is_some_and(|kind| !kind.to_ascii_lowercase().starts_with("text/"))
            {
                continue;
            }
            if let Some(text) = value["value"].as_str() {
                let (truncated, text) = truncate(
                    text,
                    options.max,
                    types
                        .get(id.as_str())
                        .is_some_and(|t| t.eq_ignore_ascii_case("text/html")),
                );
                result.insert(id.clone(), json!({"value":text,"isEncodingProblem":value["isEncodingProblem"].as_bool().unwrap_or(false),"isTruncated":truncated || value["isTruncated"] == true}));
            }
        }
    }
    Value::Object(result)
}
fn project(email: &Value, options: &Options) -> Result<Value, Error> {
    let mut row = Map::new();
    row.insert("id".into(), email["id"].clone());
    for property in &options.properties {
        let value = match property.as_str() {
            "bodyValues" => body_values(email, options),
            "bodyStructure" => part(&email[property], &options.body_properties)?,
            "textBody" | "htmlBody" | "attachments"
                if email.get(property).is_some_and(Value::is_null) =>
            {
                Value::Null
            }
            "textBody" | "htmlBody" | "attachments" => Value::Array(
                email[property]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .map(|p| part(p, &options.body_properties))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()?
                    .unwrap_or_default(),
            ),
            name if name.starts_with("header:") => header(email, name)?,
            _ => email.get(property).cloned().unwrap_or(Value::Null),
        };
        row.insert(property.clone(), value);
    }
    Ok(Value::Object(row))
}
pub fn answer(objects: &[Value], account: &str, state: &str, args: &Value) -> Result<Value, Error> {
    let options = Options::parse(args)?;
    let ids = strings(args, "ids")?;
    let mut list = Vec::new();
    let mut missing = Vec::new();
    if let Some(ids) = ids {
        let mut seen = HashSet::new();
        let index: HashMap<_, _> = objects
            .iter()
            .filter_map(|o| o["id"].as_str().map(|id| (id, o)))
            .collect();
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(object) = index.get(id.as_str()) {
                list.push(project(object, &options)?);
            } else {
                missing.push(id);
            }
        }
    } else {
        for object in objects {
            list.push(project(object, &options)?);
        }
    }
    Ok(json!({"accountId":account,"state":state,"list":list,"notFound":missing}))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{replica::Collection, storage::Store};

    fn message() -> Value {
        json!({"id":"e","subject":"Stored subject","keywords":{},"internalSecret":"never project",
            "headers":[
                {"name":"Authentication-Results","value":" first; dkim=pass\r\n"},
                {"name":"authentication-results","value":" second; spf=pass"},
                {"name":"Subject","value":"=?UTF-8?B?R3LDvMOfZQ==?="},
                {"name":"To","value":"Team: Ada <ada@example.test>, Bob <bob@example.test>;"},
                {"name":"Message-ID","value":"<one@example.test> <two@example.test>"},
                {"name":"Date","value":"Mon, 16 Nov 2026 12:00:00 +0200"},
                {"name":"List-Unsubscribe","value":"<https://example.test/unsub>, <mailto:leave@example.test>"}
            ],
            "textBody":[{"partId":"1","type":"text/plain","charset":"utf-8","headers":[{"name":"Content-Language","value":"de"}]}],
            "htmlBody":[{"partId":"2","type":"text/html"}],
            "attachments":[{"partId":"3","type":"text/plain","name":"note.txt","headers":[]}],
            "bodyStructure":{"type":"multipart/mixed","subParts":[{"type":"multipart/alternative","subParts":[{"partId":"1","type":"text/plain"},{"partId":"2","type":"text/html"}]},{"partId":"3","type":"text/plain"}]},
            "bodyValues":{"1":{"value":"\u{00e9}\u{00e9}\u{00e9}\u{00e9}","isTruncated":false,"isEncodingProblem":true},
                          "2":{"value":"<p>Hello</p><img src='long data'>","isTruncated":false},
                          "3":{"value":"text attachment","isTruncated":true}}
        })
    }
    fn one(args: Value) -> Value {
        answer(&[message()], "a", "state", &args).unwrap()["list"][0].clone()
    }

    #[test]
    fn projection_defaults_ids_validation_and_offline_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("email-get.redb");
        {
            let store = Store::open(&path).unwrap();
            store
                .apply_remote("a", Collection::Email, None, "s", &[message()], &[], true)
                .unwrap();
            store
                .apply_remote(
                    "b",
                    Collection::Email,
                    None,
                    "s",
                    &[json!({"id":"private","subject":"other"})],
                    &[],
                    true,
                )
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let mail = store.local_mail("a").unwrap();
        let response = answer(&mail.emails,"a","s",&json!({"ids":["e","private","e","missing","missing"],"properties":["subject","sender"]})).unwrap();
        assert_eq!(
            response["list"],
            json!([{"id":"e","subject":"Stored subject","sender":null}])
        );
        assert_eq!(response["notFound"], json!(["private", "missing"]));
        assert_eq!(one(json!({"properties":[]})), json!({"id":"e"}));
        let default = one(json!({}));
        assert_eq!(default["bodyValues"], json!({}));
        assert!(default.get("headers").is_none());
        assert!(default.get("internalSecret").is_none());
        assert_eq!(
            answer(&[], "a", "s", &json!({"ids":[]})).unwrap()["list"],
            json!([])
        );
        for args in [
            json!({"properties":[4]}),
            json!({"ids":[null]}),
            json!({"properties":["internalSecret"]}),
            json!({"bodyProperties":["subject"]}),
            json!({"fetchAllBodyValues":1}),
            json!({"maxBodyValueBytes":-1}),
            json!({"maxBodyValueBytes":1.5}),
            json!({"properties":["header:bad name"]}),
            json!({"properties":["header:x:all:asText"]}),
            json!({"unexpected":true}),
        ] {
            assert_eq!(answer(&[], "a", "s", &args), Err("invalidArguments"));
        }
    }
    #[test]
    fn body_modes_recursive_projection_and_utf8_truncation() {
        let text = one(
            json!({"properties":["bodyValues"],"fetchTextBodyValues":true,"maxBodyValueBytes":7}),
        );
        assert_eq!(text["bodyValues"].as_object().unwrap().len(), 1);
        assert_eq!(
            text["bodyValues"]["1"],
            json!({"value":"\u{00e9}\u{00e9}...","isTruncated":true,"isEncodingProblem":true})
        );
        let html = one(
            json!({"properties":["bodyValues"],"fetchHTMLBodyValues":true,"maxBodyValueBytes":20}),
        );
        assert_eq!(html["bodyValues"].as_object().unwrap().len(), 1);
        assert!(!html["bodyValues"]["2"]["value"]
            .as_str()
            .unwrap()
            .contains("<img"));
        let all = one(json!({"properties":["bodyValues"],"fetchAllBodyValues":true}));
        assert_eq!(all["bodyValues"].as_object().unwrap().len(), 3);
        assert_eq!(all["bodyValues"]["3"]["isTruncated"], true);
        let projected = one(
            json!({"properties":["bodyStructure","textBody"],"bodyProperties":["partId","header:Content-Language:asText"]}),
        );
        assert_eq!(
            projected["textBody"][0]["header:Content-Language:asText"],
            "de"
        );
        assert!(projected["textBody"][0].get("charset").is_none());
        assert_eq!(
            projected["bodyStructure"]["subParts"][0]["subParts"][1]["partId"],
            "2"
        );
        assert!(projected.get("bodyValues").is_none());
        let unavailable = answer(
            &[json!({"id":"e","bodyValues":null,"textBody":null})],
            "a",
            "s",
            &json!({"properties":["bodyValues","textBody"],"fetchAllBodyValues":true}),
        )
        .unwrap();
        assert!(unavailable["list"][0]["bodyValues"].is_null());
        assert!(unavailable["list"][0]["textBody"].is_null());
        let non_text = answer(&[json!({"id":"e","attachments":[{"partId":"binary","type":"application/octet-stream"}],"bodyValues":{"binary":{"value":"not a text part"}}})],"a","s",&json!({"properties":["bodyValues"],"fetchAllBodyValues":true})).unwrap();
        assert_eq!(non_text["list"][0]["bodyValues"], json!({}));
    }
    #[test]
    fn header_forms_duplicates_case_and_missing_values_use_stalwart_parser() {
        let props = [
            "headers",
            "header:AUTHENTICATION-RESULTS:all",
            "header:Authentication-Results",
            "header:subject:asText",
            "header:To:asAddresses",
            "header:To:asGroupedAddresses",
            "header:Message-ID:asMessageIds",
            "header:Date:asDate",
            "header:List-Unsubscribe:asURLs",
            "header:Missing:all",
            "header:Missing:asText",
        ];
        let row = one(json!({"properties":props}));
        assert_eq!(
            row["header:AUTHENTICATION-RESULTS:all"],
            json!([" first; dkim=pass", " second; spf=pass"])
        );
        assert_eq!(row["header:Authentication-Results"], " second; spf=pass");
        assert_eq!(row["header:subject:asText"], "Gr\u{00fc}\u{00df}e");
        assert_eq!(row["header:To:asAddresses"][0]["email"], "ada@example.test");
        assert_eq!(row["header:To:asGroupedAddresses"][0]["name"], "Team");
        assert_eq!(
            row["header:Message-ID:asMessageIds"],
            json!(["one@example.test", "two@example.test"])
        );
        assert_eq!(row["header:Date:asDate"], "2026-11-16T10:00:00Z");
        assert_eq!(
            row["header:List-Unsubscribe:asURLs"],
            json!(["https://example.test/unsub", "mailto:leave@example.test"])
        );
        assert_eq!(row["header:Missing:all"], json!([]));
        assert!(row["header:Missing:asText"].is_null());
        assert_eq!(row["headers"].as_array().unwrap().len(), 7);
    }
}
