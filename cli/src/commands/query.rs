//! `buri query`.
//!
//! `deps`, `rdeps`, `path`, `tags`, `platforms`, `sources` — questions about
//! the graph `crate::build::workspace` already loaded, answered without
//! building anything.

use crate::build::session;
use crate::build::workspace::TargetId;
use crate::commands::arguments;

/// `deps`, `rdeps`, `path`, `tags`, `platforms`, `sources`.
pub fn cmd_query(args: &arguments::Args) -> i32 {
    let s = match session::open(&args.flags) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };
    let Some(expr) = args.targets.first() else {
        eprintln!("error: `buri query` takes an expression, as in 'deps(//cmd/server)'");
        return 2;
    };
    let expr = expr.trim();
    let Some((func, rest)) = expr.split_once('(') else {
        eprintln!("error: `{expr}` is not a query");
        eprintln!("  = the forms are deps, rdeps, path, tags, platforms, sources");
        return 2;
    };
    let Some(inner) = rest.strip_suffix(')') else {
        eprintln!("error: `{expr}` is missing its closing parenthesis");
        return 2;
    };
    let arguments: Vec<&str> = inner.split(',').map(|a| a.trim()).collect();

    let lookup = |label: &str| -> Option<TargetId> {
        let path = label.strip_prefix("//")?;
        let id = s.ws.pkg_by_path(path)?;
        s.ws.targets().into_iter().find(|t| t.pkg == id)
    };

    match func.trim() {
        "deps" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            for m in s.ws.closure(t) {
                if m != t {
                    println!("{}", s.ws.label(m));
                }
            }
        }
        "rdeps" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            for other in s.ws.targets() {
                if other != t && s.ws.closure(other).contains(&t) {
                    println!("{}", s.ws.label(other));
                }
            }
        }
        // The one that earns its place: the answer to "why does the JS build
        // pull in the database layer" is an edge, and printing it is faster
        // than reading build files.
        "path" => {
            if arguments.len() != 2 {
                eprintln!("error: `path` takes two targets");
                return 2;
            }
            let (Some(from), Some(to)) = (lookup(arguments[0]), lookup(arguments[1])) else {
                eprintln!("error: no such target");
                return 2;
            };
            match s.ws.dep_path(from, to) {
                Some(path) => {
                    println!("{}", s.ws.label(path[0].0));
                    for (node, span) in path.iter().skip(1) {
                        let where_ = match span {
                            Some(sp) if !sp.is_none() => {
                                let f = s.map.get(sp.file);
                                let (line, _) = f.line_col(sp.start);
                                format!("{}:{}", f.name, line)
                            }
                            _ => "implicit".into(),
                        };
                        println!("  -> {:<22} {where_}", s.ws.label(*node));
                    }
                }
                None => println!("no path"),
            }
        }
        "tags" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            for (tag, by) in s.ws.closure_tags(t) {
                println!("{tag}  ({})", s.ws.label(by));
            }
        }
        "platforms" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            for p in s.ws.platforms(t) {
                println!("{}", p.slug());
            }
        }
        "sources" => {
            let Some(t) = lookup(arguments[0]) else {
                eprintln!("error: no target {}", arguments[0]);
                return 2;
            };
            let p = s.ws.pkg(t.pkg);
            let mut all = Vec::new();
            if let Some(lib) = &p.build.library {
                all.push("lib.buri".to_string());
                all.extend(lib.sources.iter().map(|x| x.value.clone()));
            }
            if let Some(bin) = &p.build.binary {
                all.push("main.buri".to_string());
                all.extend(bin.sources.iter().map(|x| x.value.clone()));
            }
            all.sort();
            for a in all {
                println!("{}/{a}", p.path);
            }
        }
        other => {
            eprintln!("error: there is no query `{other}`");
            eprintln!("  = the forms are deps, rdeps, path, tags, platforms, sources");
            return 2;
        }
    }
    0
}
