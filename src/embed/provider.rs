//! Turning code into vectors.
//!
//! Two providers, chosen for what they cost rather than what they score:
//!
//! * `hashing` — deterministic arithmetic over token digests. No key, no network, no
//!   model to download, and the same input always gives the same vector. That makes it
//!   the right default for tests and for a machine that has to work offline.
//! * `openai` — any server speaking `/v1/embeddings`: Ollama, TEI, vLLM, or Voyage's
//!   OpenAI-compatible surface. The dimension is whatever the model says it is.
//!
//! What embeddings are *for* here is narrow, and deliberately so: they find entry
//! points when the task text names nothing the graph already knows. They do not rank
//! the answer. A symbol's position in the result comes from the graph -- distance,
//! reference kind, centrality -- and a vector distance never enters that calculation.
//! See `anchors` in the search result: a hit is labelled with which sources agreed, and
//! agreement is the signal worth reading, not the similarity number.

use serde::{Deserialize, Serialize};

/// One vector, and what produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Embedding {
    pub values: Vec<f32>,
    pub model: String,
}

impl Embedding {
    /// Cosine similarity against another vector of the same width.
    ///
    /// Returns `None` on a width mismatch rather than a number: comparing a 768-wide
    /// vector to a 1024-wide one by truncating would return a plausible-looking answer
    /// about nothing.
    pub fn similarity(&self, other: &Embedding) -> Option<f32> {
        if self.values.len() != other.values.len() {
            return None;
        }
        Some(cosine(&self.values, &other.values))
    }

    pub fn dim(&self) -> usize {
        self.values.len()
    }
}

/// Cosine similarity of two equal-length vectors, both already unit length if they came
/// from a provider that normalises. Recomputes both norms rather than trusting the
/// caller's: one provider that forgets to normalise should still compare correctly.
pub fn cosine_public(a: &[f32], b: &[f32]) -> f32 {
    cosine(a, b)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Something that turns text into a vector.
pub trait Provider {
    /// The model's name, recorded with every vector so a store can refuse to compare
    /// across models.
    fn model(&self) -> &str;

    /// The width this provider produces. `None` when it is only known after the first
    /// call, which is the normal case for an OpenAI-compatible server.
    fn dim(&self) -> Option<usize>;

    fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>, String>;
}

/// Where a provider and its credentials come from.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSpec {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
}

impl ProviderSpec {
    /// Read the provider out of an argument and the environment, in that order.
    ///
    /// The environment names come from the tool this is modelled on, so a machine set
    /// up for it keeps working: `BCE_EMBEDDING_PROVIDER` / `BCE_EMBEDDING_MODEL`.
    /// `TINY_*` is accepted too because this crate's own tools should not need a
    /// prefix belonging to another project.
    pub fn resolve(provider: Option<&str>, model: Option<&str>) -> Result<Self, String> {
        let pick = |flag: Option<&str>, vars: (&str, &str), fallback: &str| -> String {
            flag.map(str::to_string)
                .or_else(|| std::env::var(vars.0).ok().filter(|v| !v.is_empty()))
                .or_else(|| std::env::var(vars.1).ok().filter(|v| !v.is_empty()))
                .unwrap_or_else(|| fallback.to_string())
        };

        let provider = pick(
            provider,
            ("TINY_EMBEDDING_PROVIDER", "BCE_EMBEDDING_PROVIDER"),
            "hashing",
        );

        match provider.as_str() {
            "hashing" => Ok(Self {
                model: model.unwrap_or("hashing-1024").to_string(),
                base_url: String::new(),
                api_key: None,
                provider,
            }),
            "openai" => {
                let model = model
                    .map(str::to_string)
                    .or_else(|| {
                        std::env::var("TINY_EMBEDDING_MODEL")
                            .ok()
                            .filter(|v| !v.is_empty())
                    })
                    .or_else(|| {
                        std::env::var("BCE_EMBEDDING_MODEL")
                            .ok()
                            .filter(|v| !v.is_empty())
                    })
                    .ok_or_else(|| {
                        "the openai provider needs a model: pass --model, or set \
                         TINY_EMBEDDING_MODEL. Any /v1/embeddings server works -- Ollama, \
                         TEI, vLLM."
                            .to_string()
                    })?;
                let base_url = std::env::var("TINY_EMBEDDING_BASE_URL")
                    .or_else(|_| std::env::var("BCE_EMBEDDING_BASE_URL"))
                    .unwrap_or_else(|_| "http://localhost:11434/v1".to_string());
                let api_key = std::env::var("TINY_EMBEDDING_API_KEY")
                    .or_else(|_| std::env::var("BCE_EMBEDDING_API_KEY"))
                    .or_else(|_| std::env::var("OPENAI_API_KEY"))
                    .ok()
                    .filter(|k| !k.is_empty());
                Ok(Self {
                    provider,
                    model,
                    base_url: base_url.trim_end_matches('/').to_string(),
                    api_key,
                })
            }
            other => Err(format!(
                "unknown embedding provider '{other}'. Use `hashing` (no key, offline, \
                 deterministic) or `openai` (any /v1/embeddings server)."
            )),
        }
    }

    pub fn build(&self) -> Result<Box<dyn Provider>, String> {
        match self.provider.as_str() {
            "hashing" => Ok(Box::new(HashingProvider::new(self.dim_or_default()))),
            "openai" => Ok(Box::new(OpenAiProvider::new(
                self.base_url.clone(),
                self.model.clone(),
                self.api_key.clone(),
            ))),
            other => Err(format!("unknown embedding provider '{other}'")),
        }
    }

    /// The width named in `hashing-<n>` in the model string, or the default.
    fn dim_or_default(&self) -> usize {
        self.model
            .rsplit('-')
            .next()
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(1024)
    }
}

/// Deterministic bag-of-tokens hashing into `dim` buckets.
///
/// Not a language model, and does not pretend to be one: two texts score highly when
/// they share tokens, which is lexical overlap wearing a vector's clothes. What it buys
/// is that it needs nothing -- no key, no download, no network, no GPU -- and that the
/// same text always gives the same vector, which is what makes it usable as a default
/// and in tests. A real retrieval profile is what the `openai` provider is for.
pub struct HashingProvider {
    dim: usize,
}

impl HashingProvider {
    pub fn new(dim: usize) -> Self {
        Self {
            dim: usize::max(dim, 1),
        }
    }
}

/// Split code into comparable tokens.
///
/// Lowercased and split on anything that is not a letter or digit, so
/// `GetAgencyMembers` and `get_agency_members` share `get`, `agency` and `members` --
/// which is the point, since a task text rarely spells a symbol the way the source does.
/// Camel-case is also split, so a long name contributes its parts.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;

    for ch in text.chars() {
        // `_` splits rather than joins. Joining makes `get_agency_members` one token and
        // then a camel-case task text can never match it -- which is the whole reason a
        // tokeniser that understands both spellings exists.
        if ch.is_alphanumeric() {
            let lower = ch.to_lowercase().next().unwrap_or(ch);
            // A lower->upper transition inside a run starts a new token, so
            // `getAgencyMembers` yields get, agency, members.
            if ch.is_uppercase() && prev_lower && !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            current.push(lower);
            prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        } else {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            prev_lower = false;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }

    // Single characters carry no retrieval signal and dominate a hashed space.
    out.retain(|t| t.len() > 1);
    out
}

impl Provider for HashingProvider {
    fn model(&self) -> &str {
        "hashing"
    }

    fn dim(&self) -> Option<usize> {
        Some(self.dim)
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>, String> {
        Ok(texts
            .iter()
            .map(|text| {
                let mut values = vec![0f32; self.dim];
                for token in tokenize(text) {
                    // blake3 rather than DefaultHasher: the same text must hash the same
                    // tomorrow, and DefaultHasher is not guaranteed to.
                    let digest = blake3::hash(token.as_bytes());
                    let bytes = digest.as_bytes();
                    let bucket = (u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                        as usize)
                        % self.dim;
                    // A sign from a byte the index never reads, so the sign is independent
                    // of the bucket and two tokens hashing to one bucket do not cancel
                    // more often than chance.
                    let sign = if bytes[4] & 1 == 0 { 1.0 } else { -1.0 };
                    values[bucket] += sign;
                }
                normalise(&mut values);
                Embedding {
                    values,
                    model: format!("hashing-{}", self.dim),
                }
            })
            .collect())
    }
}

/// Scale to unit length, so cosine similarity is a plain dot product downstream.
fn normalise(values: &mut [f32]) {
    let norm: f32 = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in values.iter_mut() {
            *v /= norm;
        }
    }
}

/// Any server speaking `/v1/embeddings`.
///
/// Blocking on purpose, like every other tool in this crate: the MCP handlers are plain
/// `fn` and adding a runtime here would make the sync paths a second-class citizen.
pub struct OpenAiProvider {
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl OpenAiProvider {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            api_key,
        }
    }
}

impl Provider for OpenAiProvider {
    fn model(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> Option<usize> {
        // The server decides, and it only says so once asked.
        None
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let body = serde_json::json!({ "model": self.model, "input": texts });

        let mut request = ureq::post(&format!("{}/embeddings", self.base_url))
            .header("content-type", "application/json");
        if let Some(key) = &self.api_key {
            request = request.header("authorization", format!("Bearer {key}"));
        }

        let payload: serde_json::Value = request
            .send_json(&body)
            .map_err(|e| {
                format!(
                    "embedding request to {}/embeddings failed: {e}. The openai provider \
                     needs a server that speaks /v1/embeddings -- Ollama (`ollama serve`, \
                     then --base-url http://localhost:11434/v1), TEI or vLLM.",
                    self.base_url
                )
            })?
            .into_body()
            .read_json()
            .map_err(|e| format!("embedding response was not JSON: {e}"))?;

        // The OpenAI shape: data[].embedding, in the order the inputs were given.
        let data = payload["data"]
            .as_array()
            .ok_or_else(|| format!("embedding response has no `data` array: {payload}"))?;

        let mut out: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());
        for (i, item) in data.iter().enumerate() {
            let index = item["index"].as_u64().unwrap_or(i as u64) as usize;
            let values: Vec<f32> = item["embedding"]
                .as_array()
                .ok_or_else(|| format!("embedding response item has no `embedding`: {item}"))?
                .iter()
                .filter_map(|v| v.as_f64())
                .map(|v| v as f32)
                .collect();
            if values.is_empty() {
                return Err(format!("embedding response item {index} is empty"));
            }
            out.push((index, values));
        }
        out.sort_by_key(|(i, _)| *i);

        let dims: Vec<usize> = out.iter().map(|(_, v)| v.len()).collect();
        if dims.windows(2).any(|w| w[0] != w[1]) {
            return Err(format!(
                "the server returned vectors of different widths ({dims:?}) in one \
                 response; a store cannot hold both"
            ));
        }

        Ok(out
            .into_iter()
            .map(|(_, values)| Embedding {
                values,
                model: self.model.clone(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(dim: usize) -> HashingProvider {
        HashingProvider::new(dim)
    }

    fn embed_one(p: &HashingProvider, text: &str) -> Embedding {
        p.embed(&[text.to_string()]).expect("embed").remove(0)
    }

    /// The whole reason this provider is the default: same input, same vector, always.
    #[test]
    fn the_same_text_always_gives_the_same_vector() {
        let p = provider(256);
        let a = embed_one(&p, "public class AgencyRepository { }");
        let b = embed_one(&p, "public class AgencyRepository { }");
        assert_eq!(a.values, b.values);
    }

    /// And a change anywhere in the text moves it.
    #[test]
    fn different_text_gives_a_different_vector() {
        let p = provider(256);
        let a = embed_one(&p, "GetAgencyMembers");
        let b = embed_one(&p, "DeleteAgencyMember");
        assert_ne!(a.values, b.values);
    }

    #[test]
    fn vectors_are_unit_length() {
        let p = provider(256);
        let v = embed_one(&p, "Handler Resolve");
        let norm: f32 = v.values.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "norm was {norm}");
    }

    /// Camel case is split so a task text can match the parts of a symbol name.
    #[test]
    fn camel_case_splits_into_parts() {
        let tokens = tokenize("GetAgencyMembersEndpoint");
        assert!(tokens.contains(&"get".to_string()), "{tokens:?}");
        assert!(tokens.contains(&"agency".to_string()), "{tokens:?}");
        assert!(tokens.contains(&"members".to_string()), "{tokens:?}");
    }

    /// ...and an underscore spelling of the same idea lands on the same tokens, which is
    /// how `GetAgencyMembers` is found by a task that says `agency members`.
    #[test]
    fn snake_case_and_camel_case_share_tokens() {
        assert_eq!(
            tokenize("get_agency_members"),
            tokenize("GetAgencyMembers"),
            "the two spellings must reduce to the same tokens"
        );
    }

    #[test]
    fn single_characters_are_dropped() {
        // Otherwise `a`, `b`, `c` flood the space and dominate every similarity.
        assert!(!tokenize("a b c").contains(&"a".to_string()));
    }

    /// An empty text must still produce a usable vector, not a division by zero.
    #[test]
    fn an_empty_text_is_not_a_nan() {
        let v = embed_one(&provider(64), "");
        assert!(v.values.iter().all(|x| x.is_finite()), "{:?}", v.values);
    }

    /// Width mismatch has to be visible. Truncating would return a number about nothing.
    #[test]
    fn comparing_across_widths_returns_nothing() {
        let a = Embedding {
            values: vec![1.0, 0.0],
            model: "x".into(),
        };
        let b = Embedding {
            values: vec![1.0, 0.0, 0.0],
            model: "y".into(),
        };
        assert_eq!(a.similarity(&b), None);
    }

    #[test]
    fn identical_vectors_are_maximally_similar() {
        let p = provider(128);
        let v = embed_one(&p, "Handler");
        let s = v.similarity(&v).expect("same width");
        assert!((s - 1.0).abs() < 1e-5, "similarity was {s}");
    }

    /// A token repeated must not look like two different tokens: sublinear term
    /// weighting is what stops a long symbol from matching everything that shares a word.
    #[test]
    fn repeating_a_token_does_not_double_its_weight() {
        let p = provider(256);
        let once = embed_one(&p, "handler");
        let twice = embed_one(&p, "handler handler");
        let s = once.similarity(&twice).expect("same width");
        assert!(s > 0.8, "repetition should stay similar, got {s}");
    }

    /// The default has to be the offline one, and the model string carries its width so
    /// a store can refuse a query from a different one.
    #[test]
    fn hashing_is_the_default_and_its_width_is_in_the_model_name() {
        let spec = ProviderSpec::resolve(None, None).expect("resolve");
        assert_eq!(spec.provider, "hashing");
        let e = spec
            .build()
            .unwrap()
            .embed(&["x".into()])
            .unwrap()
            .remove(0);
        assert_eq!(e.dim(), 1024);
        assert!(e.model.contains("1024"), "{}", e.model);
    }

    /// `hashing-512` in the model name picks the width, so a test can be cheap.
    #[test]
    fn the_model_name_picks_the_width() {
        let spec = ProviderSpec::resolve(Some("hashing"), Some("hashing-64")).expect("resolve");
        let e = spec
            .build()
            .unwrap()
            .embed(&["x".into()])
            .unwrap()
            .remove(0);
        assert_eq!(e.dim(), 64);
    }

    /// Asking for a model without giving one is a question, not a crash.
    #[test]
    fn the_openai_provider_without_a_model_says_what_is_missing() {
        let err = ProviderSpec::resolve(Some("openai"), None).unwrap_err();
        assert!(err.contains("needs a model"), "{err}");
    }

    #[test]
    fn an_unknown_provider_is_named_in_the_error() {
        let err = ProviderSpec::resolve(Some("cohere"), None).unwrap_err();
        assert!(err.contains("cohere"), "{err}");
    }

    #[test]
    fn no_texts_is_no_request() {
        let p = OpenAiProvider::new("http://localhost:1/v1".into(), "m".into(), None);
        assert_eq!(p.embed(&[]).expect("empty").len(), 0);
    }
}
