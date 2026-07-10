use std::borrow::Cow;
use std::collections::HashSet;

use hcl_edit::expr::Expression;
use hcl_edit::template::Element;

use crate::schema::{
    describe_expr, extract_traversal_path, LocalsRegistry, Resolution, VariablesRegistry,
};

pub enum EvalError {
    Silent,
    Message(String),
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub enum BindingKind {
    Local,
    Var,
}

impl BindingKind {
    fn prefix(self) -> &'static str {
        match self {
            BindingKind::Local => "local",
            BindingKind::Var => "var",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            BindingKind::Local => "local",
            BindingKind::Var => "variable",
        }
    }
}

pub fn binding_ref(expr: &Expression) -> Option<(BindingKind, String)> {
    let Expression::Traversal(t) = expr else {
        return None;
    };
    let path = extract_traversal_path(t)?;
    if path.len() < 2 {
        return None;
    }
    let kind = match path[0].as_str() {
        "local" => BindingKind::Local,
        "var" => BindingKind::Var,
        _ => return None,
    };
    Some((kind, path[1].clone()))
}

pub struct EvalCtx<'a> {
    pub locals: &'a LocalsRegistry,
    pub variables: &'a VariablesRegistry,
}

impl<'a> EvalCtx<'a> {
    pub fn eval(
        &self,
        module: &str,
        expr: &'a Expression,
    ) -> Result<Cow<'a, Expression>, EvalError> {
        let mut stack: HashSet<(BindingKind, String)> = HashSet::new();
        self.eval_inner(module, expr, &mut stack)
    }

    fn eval_inner(
        &self,
        module: &str,
        expr: &'a Expression,
        stack: &mut HashSet<(BindingKind, String)>,
    ) -> Result<Cow<'a, Expression>, EvalError> {
        match expr {
            Expression::Traversal(_) => {
                let Some((kind, name)) = binding_ref(expr) else {
                    return Ok(Cow::Borrowed(expr));
                };
                let (qualified, decl_module, value) = self.step(module, kind, &name)?;
                let key = (kind, qualified);
                if !stack.insert(key.clone()) {
                    return Err(EvalError::Message(format!(
                        "cyclic reference involving '{}.{name}'",
                        kind.prefix()
                    )));
                }
                let result = self.eval_inner(decl_module, value, stack);
                stack.remove(&key);
                result
            }
            Expression::StringTemplate(template) => {
                let mut rendered = String::new();
                for element in template.iter() {
                    match element {
                        Element::Literal(lit) => rendered.push_str(lit.value()),
                        Element::Interpolation(interp) => {
                            let value = self.eval_inner(module, &interp.expr, stack)?;
                            match value.as_ref() {
                                Expression::String(s) => rendered.push_str(s.value()),
                                Expression::Number(n) => {
                                    rendered.push_str(&format!("{}", **n));
                                }
                                Expression::Bool(b) => {
                                    rendered.push_str(if *b.value() { "true" } else { "false" });
                                }
                                other => {
                                    return Err(EvalError::Message(format!(
                                        "string interpolation must resolve to a string, number, or bool, got {}",
                                        describe_expr(other)
                                    )));
                                }
                            }
                        }
                        Element::Directive(_) => {
                            return Err(EvalError::Message(
                                "template directives ('%{ … }') are not supported".to_string(),
                            ));
                        }
                    }
                }
                Ok(Cow::Owned(Expression::from(rendered)))
            }
            Expression::Parenthesis(p) => self.eval_inner(module, p.inner(), stack),
            _ => Ok(Cow::Borrowed(expr)),
        }
    }

    fn step(
        &self,
        module: &str,
        kind: BindingKind,
        name: &str,
    ) -> Result<(String, &'a str, &'a Expression), EvalError> {
        let resolution = match kind {
            BindingKind::Local => self.locals.resolve(module, name),
            BindingKind::Var => self.variables.resolve(module, name),
        };
        match resolution {
            Resolution::Found(qualified) => {
                let decl = match kind {
                    BindingKind::Local => self
                        .locals
                        .get(&qualified)
                        .map(|d| (d.module.as_str(), &d.value)),
                    BindingKind::Var => self
                        .variables
                        .get(&qualified)
                        .map(|d| (d.module.as_str(), &d.value)),
                };
                match decl {
                    Some((decl_module, value)) => Ok((qualified, decl_module, value)),
                    None => Err(EvalError::Silent),
                }
            }
            Resolution::Missing => Err(EvalError::Message(format!(
                "reference to undeclared {} '{}.{name}'",
                kind.noun(),
                kind.prefix()
            ))),
            Resolution::Ambiguous(modules) => {
                let mut sorted: Vec<String> = modules.to_vec();
                sorted.sort();
                Err(EvalError::Message(format!(
                    "ambiguous reference to '{}.{name}'; declared in modules [{}] — rename one of the {}s so each is unique within its module",
                    kind.prefix(),
                    sorted.join(", "),
                    kind.noun()
                )))
            }
        }
    }
}
