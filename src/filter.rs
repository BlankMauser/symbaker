use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use syn::{punctuated::Punctuated, Expr, ExprLit, Lit, Meta, Token};

#[derive(Debug, Default)]
pub struct ModuleRules {
    pub include_regex: Vec<Regex>,
    pub exclude_regex: Vec<Regex>,
    pub include_glob: Option<GlobSet>,
    pub exclude_glob: Option<GlobSet>,
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

fn build_globset(
    specs: &[String],
    value_span: &Expr,
    kind: &str,
) -> Result<Option<GlobSet>, syn::Error> {
    if specs.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for g in specs {
        let glob = Glob::new(g).map_err(|e| {
            syn::Error::new_spanned(value_span, format!("symbaker_module: invalid {kind} glob '{g}': {e}"))
        })?;
        b.add(glob);
    }
    let set = b.build().map_err(|e| {
        syn::Error::new_spanned(value_span, format!("symbaker_module: invalid {kind} glob set: {e}"))
    })?;
    Ok(Some(set))
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
                "include_glob" => out.include_glob = build_globset(&include_glob_src, &nv.value, "include")?,
                "exclude_glob" => out.exclude_glob = build_globset(&exclude_glob_src, &nv.value, "exclude")?,
                _ => {}
            }
        }
    }

    Ok(out)
}

impl ModuleRules {
    fn included(&self, name: &str) -> bool {
        let regex_ok = if self.include_regex.is_empty() {
            true
        } else {
            self.include_regex.iter().any(|r| r.is_match(name))
        };
        let glob_ok = if let Some(set) = &self.include_glob {
            set.is_match(name)
        } else {
            true
        };
        regex_ok && glob_ok
    }

    fn excluded(&self, name: &str) -> bool {
        if self.exclude_regex.iter().any(|r| r.is_match(name)) {
            return true;
        }
        if let Some(set) = &self.exclude_glob {
            if set.is_match(name) {
                return true;
            }
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
