use regex::Regex;
use syn::{punctuated::Punctuated, Expr, ExprLit, Lit, Meta, Token};

#[derive(Debug, Default)]
pub struct ModuleRules {
    pub include_regex: Vec<Regex>,
    pub exclude_regex: Vec<Regex>,
    pub include_glob: Vec<String>,
    pub exclude_glob: Vec<String>,
    pub template: Option<String>,
    pub suffix: Option<String>,
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn validate_globs(specs: &[String], value_span: &Expr, kind: &str) -> Result<Vec<String>, syn::Error> {
    for g in specs {
        if g.contains('[') || g.contains(']') || g.contains('{') || g.contains('}') {
            return Err(syn::Error::new_spanned(
                value_span,
                format!("symbaker_module: unsupported {kind} glob '{g}' (use only '*' and '?')"),
            ));
        }
    }
    Ok(specs.to_vec())
}

fn compile_regexes(
    specs: &[String],
    value_span: &Expr,
    kind: &str,
) -> Result<Vec<Regex>, syn::Error> {
    let mut out = Vec::new();
    for r in specs {
        out.push(Regex::new(r).map_err(|e| {
            syn::Error::new_spanned(value_span, format!("symbaker_module: invalid {kind} regex '{r}': {e}"))
        })?);
    }
    Ok(out)
}

pub fn parse_module_rules(args: &Punctuated<Meta, Token![,]>) -> Result<ModuleRules, syn::Error> {
    let mut out = ModuleRules::default();
    let mut include_regex_src: Vec<String> = Vec::new();
    let mut exclude_regex_src: Vec<String> = Vec::new();
    let mut include_glob_src: Vec<String> = Vec::new();
    let mut exclude_glob_src: Vec<String> = Vec::new();

    for a in args {
        if let Meta::NameValue(nv) = a {
            let Some(key) = nv.path.get_ident().map(|i| i.to_string()) else {
                continue;
            };
            if let Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) = &nv.value {
                let v = s.value();
                match key.as_str() {
                    "include_regex" => include_regex_src.extend(parse_csv(&v)),
                    "exclude_regex" => exclude_regex_src.extend(parse_csv(&v)),
                    "include_glob" => include_glob_src.extend(parse_csv(&v)),
                    "exclude_glob" => exclude_glob_src.extend(parse_csv(&v)),
                    "template" => out.template = Some(v),
                    "suffix" => out.suffix = Some(v),
                    _ => {}
                }
            }
        }
    }

    for a in args {
        if let Meta::NameValue(nv) = a {
            let key = nv.path.get_ident().map(|i| i.to_string()).unwrap_or_default();
            match key.as_str() {
                "include_regex" => out.include_regex = compile_regexes(&include_regex_src, &nv.value, "include")?,
                "exclude_regex" => out.exclude_regex = compile_regexes(&exclude_regex_src, &nv.value, "exclude")?,
                "include_glob" => out.include_glob = validate_globs(&include_glob_src, &nv.value, "include")?,
                "exclude_glob" => out.exclude_glob = validate_globs(&exclude_glob_src, &nv.value, "exclude")?,
                _ => {}
            }
        }
    }

    Ok(out)
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut match_i) = (None::<usize>, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            pi += 1;
            match_i = ti;
        } else if let Some(star_pos) = star {
            pi = star_pos + 1;
            match_i += 1;
            ti = match_i;
        } else {
            return false;
        }
    }

    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

impl ModuleRules {
    fn included(&self, name: &str) -> bool {
        let regex_ok = if self.include_regex.is_empty() {
            true
        } else {
            self.include_regex.iter().any(|r| r.is_match(name))
        };
        let glob_ok = if self.include_glob.is_empty() {
            true
        } else {
            self.include_glob.iter().any(|g| wildcard_match(g, name))
        };
        regex_ok && glob_ok
    }

    fn excluded(&self, name: &str) -> bool {
        if self.exclude_regex.iter().any(|r| r.is_match(name)) {
            return true;
        }
        if self.exclude_glob.iter().any(|g| wildcard_match(g, name)) {
            return true;
        }
        false
    }

    pub fn should_prefix(&self, module: &str, name: &str) -> bool {
        let subject = format!("{module}::{name}");
        let include = self.included(name) || self.included(&subject);
        include && !self.excluded(name) && !self.excluded(&subject)
    }

    pub fn render_export_name(&self, prefix: &str, sep: &str, module: &str, name: &str) -> String {
        let suffix = self.suffix.as_deref().unwrap_or("");
        if let Some(tpl) = &self.template {
            return tpl
                .replace("{prefix}", prefix)
                .replace("{sep}", sep)
                .replace("{module}", module)
                .replace("{name}", name)
                .replace("{suffix}", suffix);
        }
        format!("{prefix}{sep}{name}{suffix}")
    }
}
