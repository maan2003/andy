use serde::{Deserialize, Deserializer};
use std::collections::{HashMap, HashSet};

fn empty_as_none<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    Ok(s.filter(|s| !s.is_empty()))
}

#[derive(Deserialize)]
pub struct A11yTree {
    pub windows: Vec<A11yWindow>,
}

#[derive(Deserialize)]
pub struct A11yWindow {
    pub nodes: Vec<A11yNode>,
}

#[derive(Deserialize)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Deserialize)]
pub struct A11yNode {
    pub id: i32,
    pub parent_id: Option<i32>,
    #[serde(rename = "class")]
    pub class_name: Option<String>,
    #[serde(deserialize_with = "empty_as_none")]
    pub text: Option<String>,
    #[serde(deserialize_with = "empty_as_none")]
    pub content_desc: Option<String>,
    #[serde(deserialize_with = "empty_as_none")]
    pub hint: Option<String>,
    #[serde(default)]
    pub checkable: bool,
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub clickable: bool,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub scrollable: bool,
    #[serde(default)]
    pub long_clickable: bool,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub password: bool,
    pub bounds: Bounds,
}

pub fn find_node<'a>(tree: &'a A11yTree, query: &str) -> Option<&'a A11yNode> {
    for window in &tree.windows {
        for node in &window.nodes {
            if node.text.as_deref() == Some(query) || node.content_desc.as_deref() == Some(query) {
                return Some(node);
            }
        }
    }
    None
}

pub fn render_text(tree: &A11yTree) -> String {
    let mut lines = Vec::new();

    for window in &tree.windows {
        if window.nodes.is_empty() {
            continue;
        }

        let mut children_map: HashMap<i32, Vec<usize>> = HashMap::new();
        let mut root_idx = None;

        for (idx, node) in window.nodes.iter().enumerate() {
            if let Some(pid) = node.parent_id {
                children_map.entry(pid).or_default().push(idx);
            } else {
                root_idx = Some(idx);
            }
        }

        if let Some(ri) = root_idx {
            render_node(&window.nodes, ri, 0, None, &children_map, &mut lines);
        }
    }

    lines.join("\n")
}

fn is_interesting(node: &A11yNode) -> bool {
    node.text.is_some()
        || node.content_desc.is_some()
        || node.hint.is_some()
        || node.clickable
        || node.scrollable
        || node.checkable
        || node.long_clickable
        || node.focused
        || node.selected
}

fn short_class(class: &Option<String>) -> Option<&str> {
    let cls = class.as_deref()?;
    let name = cls.rsplit('.').next().unwrap_or(cls);
    match name {
        "ViewGroup" | "FrameLayout" | "LinearLayout" | "RelativeLayout" | "ConstraintLayout" => {
            None
        }
        _ => Some(name),
    }
}

fn render_node(
    nodes: &[A11yNode],
    idx: usize,
    depth: usize,
    parent_texts: Option<&HashSet<&str>>,
    children_map: &HashMap<i32, Vec<usize>>,
    lines: &mut Vec<String>,
) {
    let node = &nodes[idx];
    let children = children_map.get(&node.id);

    let only_text = node.text.is_some()
        && node.content_desc.is_none()
        && node.hint.is_none()
        && !node.clickable
        && !node.scrollable
        && !node.checkable
        && !node.long_clickable
        && !node.focused
        && !node.selected;

    if only_text {
        if let (Some(text), Some(pt)) = (&node.text, parent_texts) {
            if pt.contains(text.as_str()) {
                if let Some(child_indices) = children {
                    for &ci in child_indices {
                        render_node(nodes, ci, depth, None, children_map, lines);
                    }
                }
                return;
            }
        }
    }

    if is_interesting(node) {
        let indent = "  ".repeat(depth);
        let cls = short_class(&node.class_name).unwrap_or("View");
        let b = &node.bounds;
        let mut line = format!("{indent}{cls}");

        if let Some(text) = &node.text {
            line.push_str(&format!(" \"{}\"", text.replace('\n', "\\n")));
        }
        if let Some(desc) = &node.content_desc {
            line.push_str(&format!(" [{}]", desc.replace('\n', "\\n")));
        }
        if let Some(hint) = &node.hint {
            line.push_str(&format!(" hint=\"{}\"", hint.replace('\n', "\\n")));
        }

        let mut flags = Vec::new();
        let implicitly_clickable = cls == "Button";
        if node.clickable && !implicitly_clickable {
            flags.push("clickable");
        }
        if !node.clickable && implicitly_clickable {
            flags.push("NOT-clickable");
        }
        if node.long_clickable {
            flags.push("long-clickable");
        }
        if node.scrollable {
            flags.push("scrollable");
        }
        if node.checkable {
            flags.push("checkable");
        }
        if node.checked {
            flags.push("checked");
        }
        if node.focused {
            flags.push("focused");
        }
        if node.selected {
            flags.push("selected");
        }
        if node.password {
            flags.push("password");
        }
        if !flags.is_empty() {
            line.push_str(&format!(" {}", flags.join(" ")));
        }
        line.push_str(&format!(" ({},{},{},{})", b.left, b.top, b.right, b.bottom));
        lines.push(line);

        let mut new_parent_texts = HashSet::new();
        if let Some(text) = &node.text {
            new_parent_texts.insert(text.as_str());
        }
        if let Some(desc) = &node.content_desc {
            new_parent_texts.insert(desc.as_str());
        }
        if let Some(child_indices) = children {
            for &ci in child_indices {
                render_node(
                    nodes,
                    ci,
                    depth + 1,
                    Some(&new_parent_texts),
                    children_map,
                    lines,
                );
            }
        }
    } else if let Some(child_indices) = children {
        for &ci in child_indices {
            render_node(nodes, ci, depth, None, children_map, lines);
        }
    }
}
