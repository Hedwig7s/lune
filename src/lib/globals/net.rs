use std::collections::HashMap;

use mlua::prelude::*;

use crate::utils::{net::get_request_user_agent_header, table_builder::TableBuilder};

pub fn create(lua: &Lua) -> LuaResult<()> {
    lua.globals().raw_set(
        "net",
        TableBuilder::new(lua)?
            .with_function("jsonEncode", net_json_encode)?
            .with_function("jsonDecode", net_json_decode)?
            .with_async_function("request", net_request)?
            .build_readonly()?,
    )
}

fn net_json_encode(_: &Lua, (val, pretty): (LuaValue, Option<bool>)) -> LuaResult<String> {
    if let Some(true) = pretty {
        serde_json::to_string_pretty(&val).map_err(LuaError::external)
    } else {
        serde_json::to_string(&val).map_err(LuaError::external)
    }
}

fn net_json_decode(lua: &Lua, json: String) -> LuaResult<LuaValue> {
    let json: serde_json::Value = serde_json::from_str(&json).map_err(LuaError::external)?;
    lua.to_value(&json)
}

async fn net_request<'lua>(lua: &'lua Lua, config: LuaValue<'lua>) -> LuaResult<LuaTable<'lua>> {
    // Extract stuff from config and make sure its all valid
    let (url, method, headers, body) = match config {
        LuaValue::String(s) => {
            let url = s.to_string_lossy().to_string();
            let method = "GET".to_string();
            Ok((url, method, HashMap::new(), None))
        }
        LuaValue::Table(tab) => {
            // Extract url
            let url = match tab.raw_get::<_, LuaString>("url") {
                Ok(config_url) => Ok(config_url.to_string_lossy().to_string()),
                Err(_) => Err(LuaError::RuntimeError(
                    "Missing 'url' in request config".to_string(),
                )),
            }?;
            // Extract method
            let method = match tab.raw_get::<_, LuaString>("method") {
                Ok(config_method) => config_method.to_string_lossy().trim().to_ascii_uppercase(),
                Err(_) => "GET".to_string(),
            };
            // Extract headers
            let headers = match tab.raw_get::<_, LuaTable>("headers") {
                Ok(config_headers) => {
                    let mut lua_headers = HashMap::new();
                    for pair in config_headers.pairs::<LuaString, LuaString>() {
                        let (key, value) = pair?.to_owned();
                        lua_headers.insert(key, value);
                    }
                    lua_headers
                }
                Err(_) => HashMap::new(),
            };
            // Extract body
            let body = match tab.raw_get::<_, LuaString>("body") {
                Ok(config_body) => Some(config_body.as_bytes().to_owned()),
                Err(_) => None,
            };
            Ok((url, method, headers, body))
        }
        value => Err(LuaError::RuntimeError(format!(
            "Invalid request config - expected string or table, got {}",
            value.type_name()
        ))),
    }?;
    // Convert method string into proper enum
    let method = method.trim().to_ascii_uppercase();
    let method = match method.as_ref() {
        "GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" | "PATCH" => Ok(&method),
        _ => Err(LuaError::RuntimeError(format!(
            "Invalid request config method '{}'",
            &method
        ))),
    }?;
    // Create and send the request
    let mut request = ureq::request(method, &url);
    for (header, value) in headers {
        request = request.set(header.to_str()?, value.to_str()?);
    }
    let response = request
        .set("User-Agent", &get_request_user_agent_header()) // Always force user agent
        .send_bytes(&body.unwrap_or_default());
    match response {
        Ok(res) | Err(ureq::Error::Status(_, res)) => {
            // Extract status, headers
            let res_status = res.status();
            let res_status_text = res.status_text().to_owned();
            let res_header_names = &res.headers_names();
            let res_headers = res_header_names
                .iter()
                .map(|name| (name.to_owned(), res.header(name).unwrap().to_owned()))
                .collect::<HashMap<String, String>>();
            // Read response bytes
            let mut res_bytes = Vec::new();
            res.into_reader().read_to_end(&mut res_bytes)?;
            // Construct and return a readonly lua table with results
            TableBuilder::new(lua)?
                .with_value("ok", (200..300).contains(&res_status))?
                .with_value("statusCode", res_status)?
                .with_value("statusMessage", res_status_text)?
                .with_value("headers", res_headers)?
                .with_value("body", lua.create_string(&res_bytes)?)?
                .build_readonly()
        }
        Err(e) => Err(LuaError::external(e)),
    }
}
