//! The shapes of the Debug Adapter Protocol.
//!
//! Only the parts the editor needs are here: the breakpoints, the frames of
//! the stack, the groups of variables and the variables themselves. The rest
//! of the protocol is the adapter's business.

use serde::{Deserialize, Serialize};

/// A file of the program that is being debugged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The path of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The name of the file, when the path alone is not enough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A breakpoint the editor asks the adapter for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceBreakpoint {
    /// The line the program is to stop at.
    pub line: u32,
    /// The file the line is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

/// A breakpoint the adapter says it has.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakpoint {
    /// The number the adapter gave the breakpoint.
    #[serde(default)]
    pub id: Option<i64>,
    /// Whether the breakpoint really is where it was asked to be.
    #[serde(default)]
    pub verified: bool,
    /// The line the breakpoint ended up on.
    #[serde(default)]
    pub line: Option<u32>,
    /// What the adapter says about it.
    #[serde(default)]
    pub message: Option<String>,
}

/// One frame of the stack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackFrame {
    /// The number the frames of the variables are asked for by.
    pub id: i64,
    /// The name of the function of the frame.
    #[serde(default)]
    pub name: String,
    /// The line the frame is on.
    #[serde(default)]
    pub line: Option<u32>,
    /// The column the frame is on.
    #[serde(default)]
    pub column: Option<u32>,
    /// The file the frame is in.
    #[serde(default)]
    pub source: Option<Source>,
}

/// A group of variables.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    /// The name of the group.
    pub name: String,
    /// The number the variables of the group are asked for by.
    #[serde(default)]
    pub variables_reference: i64,
    /// Whether the variables of the group are expensive to ask for.
    #[serde(default)]
    pub expensive: bool,
}

/// One variable of a frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    /// The name of the variable.
    pub name: String,
    /// The value of the variable, as the adapter says it.
    #[serde(default)]
    pub value: String,
    /// The type of the variable, when the adapter says it.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    /// The number the children of the variable are asked for by.
    #[serde(default)]
    pub variables_reference: i64,
}

impl Variable {
    /// The variable in one line, the way the panel shows it.
    pub fn line(&self) -> String {
        match self.type_.as_ref() {
            Some(kind) => format!("{}: {} = {}", self.name, kind, self.value),
            None => format!("{} = {}", self.name, self.value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_variable_is_read_with_its_type() {
        let variable: Variable = serde_json::from_str(
            r#"{"name":"count","value":"3","type":"i32","variablesReference":0}"#,
        )
        .expect("the variable is read");
        assert_eq!(variable.name, "count");
        assert_eq!(variable.value, "3");
        assert_eq!(variable.type_.as_deref(), Some("i32"));
        assert_eq!(variable.line(), "count: i32 = 3");
    }

    #[test]
    fn a_variable_without_a_type_still_says_something() {
        let variable: Variable =
            serde_json::from_str(r#"{"name":"name","value":"greeting"}"#)
                .expect("the variable is read");
        assert_eq!(variable.type_, None);
        assert_eq!(variable.line(), "name = greeting");
    }

    #[test]
    fn a_frame_is_read_with_its_file() {
        let frame: StackFrame = serde_json::from_str(
            r#"{"id":7,"name":"main","line":12,"column":3,"source":{"path":"C:/main.rs"}}"#,
        )
        .expect("the frame is read");
        assert_eq!(frame.id, 7);
        assert_eq!(frame.name, "main");
        assert_eq!(frame.line, Some(12));
        assert_eq!(
            frame.source.as_ref().and_then(|source| source.path.as_deref()),
            Some("C:/main.rs")
        );
    }

    #[test]
    fn a_scope_is_read_with_its_number() {
        let scope: Scope = serde_json::from_str(
            r#"{"name":"Locals","variablesReference":10,"expensive":false}"#,
        )
        .expect("the scope is read");
        assert_eq!(scope.name, "Locals");
        assert_eq!(scope.variables_reference, 10);
    }

    #[test]
    fn a_breakpoint_that_was_verified_is_read() {
        let breakpoint: Breakpoint =
            serde_json::from_str(r#"{"id":3,"verified":true,"line":9}"#)
                .expect("the breakpoint is read");
        assert_eq!(breakpoint.id, Some(3));
        assert!(breakpoint.verified);
        assert_eq!(breakpoint.line, Some(9));
    }
}
