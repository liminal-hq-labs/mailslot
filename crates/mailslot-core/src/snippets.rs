// Copy-ready command snippets and the agent prompt.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

/// What the snippets need to know about a share.
#[derive(Clone, Debug, Default)]
pub struct SnippetContext {
	/// The drop URL including the token and trailing slash.
	pub drop_url: String,
	/// The `mailslot://` link for `mailslot-cli`.
	pub link: String,
	/// Largest accepted file in bytes.
	pub max_bytes: u64,
	/// Human-readable expiry such as `in 1 hour`, or `never`.
	pub expires: String,
	/// Public-key pin when the share uses self-signed HTTPS.
	pub tls_pin: Option<String>,
}

fn quote_sh(s: &str) -> String {
	format!("'{}'", s.replace('\'', "'\\''"))
}

/// A `curl -T` command for a file.
pub fn curl(ctx: &SnippetContext, file: &str) -> String {
	let pin = ctx
		.tls_pin
		.as_ref()
		.map(|p| format!(" -k --pinnedpubkey {}", quote_sh(&format!("sha256//{p}"))))
		.unwrap_or_default();
	format!(
		"curl --fail{pin} -T {} {}",
		quote_sh(file),
		quote_sh(&format!("{}{}", ctx.drop_url, encode(file)))
	)
}

/// A PowerShell `Invoke-WebRequest` command for a file.
pub fn powershell(ctx: &SnippetContext, file: &str) -> String {
	let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
	format!(
		"Invoke-WebRequest -Method Put -InFile {} -Uri {}",
		q(file),
		q(&format!("{}{}", ctx.drop_url, encode(file)))
	)
}

/// A `wget --method=PUT` command for a file.
pub fn wget(ctx: &SnippetContext, file: &str) -> String {
	let pin = ctx
		.tls_pin
		.as_ref()
		.map(|_| " --no-check-certificate")
		.unwrap_or_default();
	format!(
		"wget{pin} --method=PUT --body-file={} {} -O -",
		quote_sh(file),
		quote_sh(&format!("{}{}", ctx.drop_url, encode(file)))
	)
}

/// A `mailslot-cli send` command for a file.
pub fn cli(ctx: &SnippetContext, file: &str) -> String {
	format!(
		"mailslot-cli send {} {}",
		quote_sh(file),
		quote_sh(&ctx.link)
	)
}

fn encode(name: &str) -> String {
	percent_encoding::utf8_percent_encode(name, crate::proto::NAME_SET).to_string()
}

/// The default agent prompt template. Placeholders: `{url}`, `{curl}`, `{cli}`, `{max_mb}`, `{expires}`.
pub const AGENT_PROMPT_TEMPLATE: &str = "\
Please send me the file(s) by uploading them to this private drop link:

{url}

Upload with curl (replace FILE with the file name):

{curl}

Rules:
- The size limit is {max_mb} MB per file. Split larger files or tell me.
- The link expires {expires}.
- Before uploading, run `sha256sum FILE` and print the hash. The server returns its own SHA-256 in the `X-Mailslot-Sha256` response header; confirm they match.
- If an upload fails, retry up to 3 times with a pause between attempts. If `mailslot-cli` is available, `{cli}` resumes interrupted uploads.
- Do not share this link anywhere else.
";

/// The agent prompt rendered from the default template.
pub fn agent_prompt(ctx: &SnippetContext) -> String {
	agent_prompt_with(AGENT_PROMPT_TEMPLATE, ctx)
}

/// The agent prompt rendered from a custom template.
pub fn agent_prompt_with(template: &str, ctx: &SnippetContext) -> String {
	template
		.replace("{url}", &ctx.drop_url)
		.replace("{curl}", &curl(ctx, "FILE"))
		.replace("{cli}", &cli(ctx, "FILE"))
		.replace("{max_mb}", &(ctx.max_bytes / (1024 * 1024)).to_string())
		.replace("{expires}", &ctx.expires)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ctx() -> SnippetContext {
		SnippetContext {
			drop_url: "http://10.0.0.2:8931/tok/".into(),
			link: "mailslot://10.0.0.2:8931/tok".into(),
			max_bytes: 200 << 20,
			expires: "in 1 hour".into(),
			tls_pin: None,
		}
	}

	#[test]
	fn curl_quotes_and_encodes() {
		let s = curl(&ctx(), "my file.zip");
		assert_eq!(
			s,
			"curl --fail -T 'my file.zip' 'http://10.0.0.2:8931/tok/my%20file%2Ezip'"
				.replace("%2E", ".")
		);
	}

	#[test]
	fn pinned_curl_uses_pinnedpubkey() {
		let mut c = ctx();
		c.tls_pin = Some("abc=".into());
		assert!(curl(&c, "f").contains("--pinnedpubkey 'sha256//abc='"));
	}

	#[test]
	fn prompt_mentions_limits() {
		let p = agent_prompt(&ctx());
		assert!(p.contains("200 MB") && p.contains("in 1 hour") && p.contains("sha256sum"));
	}
}
