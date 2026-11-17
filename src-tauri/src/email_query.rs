//! Local Email/query contract, following Stalwart's filter and window semantics.
//! Reference: stalwartlabs/stalwart e7cde9b3fac32d2a254ef782efec56f2373ec5d4,
//! crates/jmap/src/email/query/filter.rs and crates/jmap/src/api/query.rs.
//! Storage access is deliberately implemented against our account-local replica.
use serde_json::Value;

type Error = &'static str;

pub fn validate(args: &Value) -> Result<(), Error> {
    if !args.is_object() {
        return Err("invalidArguments");
    }
    filter(&args["filter"], 0)?;
    for name in ["position", "anchorOffset"] {
        if let Some(value) = args.get(name).filter(|v| !v.is_null()) {
            if value.as_i64().and_then(|n| i32::try_from(n).ok()).is_none() {
                return Err("invalidArguments");
            }
        }
    }
    if let Some(value) = args.get("limit").filter(|v| !v.is_null()) {
        if value.as_u64().and_then(|n| u32::try_from(n).ok()).is_none() {
            return Err("invalidArguments");
        }
    }
    if let Some(value) = args.get("anchor").filter(|v| !v.is_null()) {
        if !value.is_string() {
            return Err("invalidArguments");
        }
    }
    for name in ["collapseThreads", "calculateTotal"] {
        if let Some(value) = args.get(name).filter(|v| !v.is_null()) {
            if !value.is_boolean() {
                return Err("invalidArguments");
            }
        }
    }
    if let Some(sorts) = args.get("sort").filter(|v| !v.is_null()) {
        for sort in sorts.as_array().ok_or("invalidArguments")? {
            let object = sort.as_object().ok_or("invalidArguments")?;
            let property = sort["property"].as_str().ok_or("invalidArguments")?;
            if ![
                "receivedAt",
                "sentAt",
                "size",
                "subject",
                "id",
                "from",
                "to",
                "hasKeyword",
            ]
            .contains(&property)
            {
                return Err("unsupportedSort");
            }
            if let Some(value) = object.get("isAscending") {
                if !value.is_boolean() {
                    return Err("invalidArguments");
                }
            }
            if object.get("collation").is_some_and(|v| !v.is_null()) {
                return Err("unsupportedSort");
            }
            if property == "hasKeyword" && !sort["keyword"].is_string() {
                return Err("unsupportedSort");
            }
        }
    }
    Ok(())
}

fn filter(value: &Value, depth: usize) -> Result<(), Error> {
    if depth > 64 {
        return Err("invalidArguments");
    }
    if value.is_null() {
        return Ok(());
    }
    let object = value.as_object().ok_or("invalidArguments")?;
    if let Some(operator) = object.get("operator") {
        if !["AND", "OR", "NOT"].contains(&operator.as_str().ok_or("invalidArguments")?) {
            return Err("unsupportedFilter");
        }
        if object.keys().any(|k| k != "operator" && k != "conditions") {
            return Err("invalidArguments");
        }
        for child in value["conditions"].as_array().ok_or("invalidArguments")? {
            if child.is_null() {
                return Err("invalidArguments");
            }
            filter(child, depth + 1)?;
        }
        return Ok(());
    }
    for (key, value) in object {
        let valid = match key.as_str() {
            "inMailbox"
            | "hasKeyword"
            | "notKeyword"
            | "allInThreadHaveKeyword"
            | "someInThreadHaveKeyword"
            | "noneInThreadHaveKeyword"
            | "before"
            | "after"
            | "subject"
            | "from"
            | "to"
            | "cc"
            | "bcc"
            | "body"
            | "text" => value.is_string(),
            "inMailboxOtherThan" => value
                .as_array()
                .is_some_and(|ids| ids.iter().all(Value::is_string)),
            "hasAttachment" => value.is_boolean(),
            "minSize" | "maxSize" => value.as_u64().and_then(|n| u32::try_from(n).ok()).is_some(),
            _ => return Err("unsupportedFilter"),
        };
        if !valid {
            return Err("invalidArguments");
        }
    }
    Ok(())
}

/// Apply the query window after filtering, sorting and thread collapse.
pub fn position(args: &Value, emails: &[&Value]) -> Result<usize, Error> {
    if let Some(anchor) = args["anchor"].as_str() {
        let index = emails
            .iter()
            .position(|email| email["id"] == anchor)
            .ok_or("anchorNotFound")?;
        return Ok((index as i64 + args["anchorOffset"].as_i64().unwrap_or(0)).max(0) as usize);
    }
    let requested = args["position"].as_i64().unwrap_or(0);
    Ok(if requested < 0 {
        emails
            .len()
            .saturating_sub(requested.unsigned_abs() as usize)
    } else {
        requested as usize
    })
}
