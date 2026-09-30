use crate::error::XhlError;
use crate::http::Client as HttpClient;
use regex::Regex;
use std::collections::HashMap;

pub fn source_priority(url: &str) -> i32 {
    if url.contains("/responsive-web/client-web/") {
        2
    } else if url.contains("/x-web/") {
        1
    } else {
        0
    }
}

pub fn parse_operations(text: &str, source_url: &str, out: &mut HashMap<String, (String, i32)>) {
    let priority = source_priority(source_url);

    // Dua regex produksi terverifikasi dari twscrape
    let re1 = Regex::new(r#"queryId:[`"](.+?)[`"].+?operationName:[`"](.+?)[`"]"#).unwrap();
    let re2 =
        Regex::new(r#"params:\{id:[`"]([^`"]+)[`"].+?name:[`"]([^`"]+)[`"].+?operationKind:[`"]"#)
            .unwrap();

    let mut add = |op_name: String, op_id: String| {
        let entry = out.entry(op_name).or_insert((op_id.clone(), priority));
        if entry.0 != op_id && priority > entry.1 {
            *entry = (op_id, priority);
        }
    };

    for cap in re1.captures_iter(text) {
        if let (Some(id), Some(name)) = (cap.get(1), cap.get(2)) {
            add(name.as_str().to_string(), id.as_str().to_string());
        }
    }

    for cap in re2.captures_iter(text) {
        if let (Some(id), Some(name)) = (cap.get(1), cap.get(2)) {
            add(name.as_str().to_string(), id.as_str().to_string());
        }
    }
}

/// Ambil daftar operasi GraphQL dari bundle web X.
///
/// `headers` wajib berisi cookie sesi: halaman `/home` versi anonim tidak
/// mengirim daftar script yang memuat operasi.
pub async fn discover(
    client: &HttpClient,
    headers: &crate::http::client::HeaderMap,
) -> Result<HashMap<String, String>, XhlError> {
    let pages = ["https://x.com/xdevelopers", "https://x.com/home"];
    let mut script_urls = Vec::new();

    let script_re = Regex::new(r#"(?:src|href)="([^"]+?\.js)""#).unwrap();

    // Satu fetch per halaman: halaman yang sama diminta dua kali sebelumnya.
    for url in pages {
        match client.get(url, headers).await {
            Ok(resp) => {
                tracing::debug!(
                    url,
                    status = resp.status,
                    bytes = resp.body.len(),
                    "discovery: halaman diterima"
                );
                collect_script_urls(&resp.body, &script_re, &mut script_urls);
            }
            Err(e) => {
                tracing::debug!(url, error = %e, "discovery: halaman gagal diambil");
            }
        }
    }

    script_urls.sort();
    script_urls.dedup();

    // Bundle diunduh konkuren: puluhan file × ~500 ms bila berurutan menjadi
    // belasan detik; konkuren membawa total ke sekitar satu round-trip.
    // Dapat diatur `XHL_CONFIG`: `discovery_concurrency`.
    const FETCH_CONCURRENCY_DEFAULT: usize = crate::native::config::defaults::DISCOVERY_CONCURRENCY;
    let fetch_concurrency =
        crate::native::config_usize("discovery_concurrency", FETCH_CONCURRENCY_DEFAULT).max(1);
    let mut ops: HashMap<String, (String, i32)> = HashMap::new();

    for chunk in script_urls.chunks(fetch_concurrency) {
        let mut set: tokio::task::JoinSet<(String, Option<String>)> = tokio::task::JoinSet::new();
        for url in chunk {
            let client = client.clone();
            let headers = headers.clone();
            let url = url.clone();
            set.spawn(async move {
                let body = client.get(&url, &headers).await.ok().map(|r| r.body);
                (url, body)
            });
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((url, Some(body))) => parse_operations(&body, &url, &mut ops),
                Ok((_url, None)) => {}
                Err(e) => tracing::debug!(error = %e, "discovery: task bundle gagal"),
            }
        }
    }

    Ok(ops.into_iter().map(|(k, (v, _))| (k, v)).collect())
}

/// Kumpulkan URL bundle relevan dari HTML halaman.
fn collect_script_urls(html: &str, script_re: &Regex, out: &mut Vec<String>) {
    for cap in script_re.captures_iter(html) {
        if let Some(m) = cap.get(1) {
            let s = m.as_str();
            let relevant = (s.contains("/responsive-web/client-web/") || s.contains("/x-web/"))
                && !s.contains("/i18n/")
                && !s.contains("/icons/")
                && !s.contains("react-syntax-highlighter");
            if relevant {
                out.push(s.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_operations_dua_regex_dan_prioritas() {
        let bundle1 = r#"
            // Low priority bundle
            {queryId:"low123",operationName:"SearchTimeline"}
            {params:{id:"low456",name:"TweetDetail",operationKind:"query"}}
        "#;
        let bundle2 = r#"
            // High priority bundle
            {queryId:"high123",operationName:"SearchTimeline"}
        "#;

        let mut out = HashMap::new();
        parse_operations(bundle1, "https://abs.twimg.com/x-web/vendor.js", &mut out);
        assert_eq!(out.get("SearchTimeline").unwrap().0, "low123");
        assert_eq!(out.get("TweetDetail").unwrap().0, "low456");

        // Override dengan prioritas lebih tinggi (/responsive-web/client-web/)
        parse_operations(
            bundle2,
            "https://abs.twimg.com/responsive-web/client-web/main.js",
            &mut out,
        );
        assert_eq!(
            out.get("SearchTimeline").unwrap().0,
            "high123",
            "prioritas responsive-web harus menang"
        );
        assert_eq!(
            out.get("TweetDetail").unwrap().0,
            "low456",
            "operasi yang tidak ada di bundle2 tetap ada"
        );
    }
}
