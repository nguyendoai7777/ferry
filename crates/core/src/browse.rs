//! Dựng cây thư mục từ `GET /list` cho trình duyệt điện thoại (`PROTOCOL.md` 4.6, W-4): hàm thuần, không IO.
//!
//! Thư mục = các tiền tố của `LibraryItem::dir` (`/` phân cách, `""` = gốc nơi lưu); `dir = None` = nhóm "Khác"
//! (chỉ xem / tải, không làm đích). Thư mục rỗng không tồn tại trên điện thoại ⇒ "Thư mục mới" là thư mục ảo phía PC
//! ([`Entry::is_virtual`]) cho tới khi có tệp đầu tiên gửi vào.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::protocol::LibraryItem;

/// Một thư mục trong cây trái.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirNode {
    pub name: String,
    /// Đường đầy đủ từ gốc, `/` phân cách (`"Chuyến đi/Ngày 1"`).
    pub path: String,
    pub is_virtual: bool,
    pub children: Vec<DirNode>,
}

/// Cây thư mục: các thư mục cấp 1 dưới gốc + có nhóm "Khác" hay không.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    pub roots: Vec<DirNode>,
    pub has_other: bool,
}

/// Nơi đang mở trong trình duyệt.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    /// `""` = gốc nơi lưu.
    Dir(String),
    /// Video có `dir = null`.
    Other,
}

impl Location {
    pub fn root() -> Location {
        Location::Dir(String::new())
    }

    /// Có gửi vào đây được không ("Khác" thì không).
    pub fn is_destination(&self) -> bool {
        matches!(self, Location::Dir(_))
    }

    /// Thư mục cha (`Lên`); gốc và "Khác" ⇒ `None`.
    pub fn parent(&self) -> Option<Location> {
        match self {
            Location::Dir(p) if !p.is_empty() => Some(Location::Dir(parent_dir(p).to_owned())),
            _ => None,
        }
    }
}

/// `"a/b"` ⇒ `"a"`; `"a"` ⇒ `""`.
pub fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}

/// Ghép `parent` + `name` (`""` + `"a"` ⇒ `"a"`).
pub fn join_dir(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") }
}

/// Các đoạn breadcrumb (không kể gốc): `"a/b"` ⇒ `[("a","a"), ("b","a/b")]`.
pub fn breadcrumb(path: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut acc = String::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        acc = join_dir(&acc, seg);
        out.push((seg.to_owned(), acc.clone()));
    }
    out
}

/// Chuẩn hoá tên thư mục người dùng gõ: bỏ khoảng trắng đầu / cuối, không rỗng, không `/` `\` `..`.
pub fn sanitize_folder_name(name: &str) -> Option<String> {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".." || n.contains(['/', '\\']) {
        return None;
    }
    Some(n.to_owned())
}

/// Đường thư mục (đã chuẩn hoá) của mục, `None` nếu thuộc "Khác".
fn item_dir(item: &LibraryItem) -> Option<&str> {
    item.dir.as_deref().map(|d| d.trim_matches('/'))
}

/// Mọi tiền tố của `path` (`"a/b/c"` ⇒ `"a"`, `"a/b"`, `"a/b/c"`).
fn prefixes(path: &str) -> impl Iterator<Item = (usize, &str)> {
    path.match_indices('/').map(|(i, _)| i).chain(std::iter::once(path.len())).map(|end| (end, &path[..end]))
}

/// Dựng cây từ danh sách + thư mục ảo (đường đầy đủ). Anh em xếp theo tên (không phân biệt hoa thường).
pub fn build_tree(items: &[LibraryItem], virtual_dirs: &[String]) -> Tree {
    // path → is_virtual (thật thắng ảo)
    let mut dirs: BTreeMap<String, bool> = BTreeMap::new();
    let mut has_other = false;
    for it in items {
        match item_dir(it) {
            None => has_other = true,
            Some("") => {}
            Some(d) => {
                for (_, p) in prefixes(d) {
                    dirs.insert(p.to_owned(), false);
                }
            }
        }
    }
    for v in virtual_dirs {
        let v = v.trim_matches('/');
        if v.is_empty() {
            continue;
        }
        for (_, p) in prefixes(v) {
            dirs.entry(p.to_owned()).or_insert(true);
        }
    }
    fn children_of(parent: &str, dirs: &BTreeMap<String, bool>) -> Vec<DirNode> {
        let mut out: Vec<DirNode> = dirs
            .iter()
            .filter(|(p, _)| parent_dir(p) == parent && p.as_str() != parent)
            .map(|(p, v)| DirNode {
                name: p.rsplit_once('/').map_or(p.as_str(), |(_, n)| n).to_owned(),
                path: p.clone(),
                is_virtual: *v,
                children: children_of(p, dirs),
            })
            .collect();
        out.sort_by_key(|n| n.name.to_lowercase());
        out
    }
    Tree { roots: children_of("", &dirs), has_other }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    Video,
}

/// Một dòng của bảng chi tiết.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    pub name: String,
    /// Thư mục: đường đầy đủ để vào. Video: đường thư mục chứa.
    pub path: String,
    pub modified: Option<i64>,
    pub size: u64,
    /// Đuôi viết hoa (`MP4`) hoặc rỗng; thư mục để rỗng (UI hiện "Thư mục").
    pub ext: String,
    pub is_virtual: bool,
    /// Chỉ số trong danh sách `items` (video).
    pub item: Option<usize>,
}

/// Đuôi tệp viết hoa (`clip.mp4` ⇒ `MP4`).
pub fn file_ext(name: &str) -> String {
    name.rsplit_once('.').filter(|(stem, ext)| !stem.is_empty() && !ext.is_empty()).map_or_else(String::new, |(_, e)| e.to_uppercase())
}

/// Nội dung trực tiếp của một nơi: thư mục con + video. `query` lọc theo tên (không phân biệt hoa thường).
pub fn list_entries(items: &[LibraryItem], virtual_dirs: &[String], at: &Location, query: &str) -> Vec<Entry> {
    let q = query.trim().to_lowercase();
    let keep = |name: &str| q.is_empty() || name.to_lowercase().contains(&q);
    let mut out = Vec::new();
    match at {
        Location::Other => {
            for (i, it) in items.iter().enumerate().filter(|(_, it)| it.dir.is_none()) {
                if keep(&it.name) {
                    out.push(video_entry(i, it, ""));
                }
            }
        }
        Location::Dir(dir) => {
            let tree = build_tree(items, virtual_dirs);
            let level = find_children(&tree, dir);
            for node in level {
                if !keep(&node.name) {
                    continue;
                }
                let prefix = format!("{}/", node.path);
                let modified = items
                    .iter()
                    .filter(|it| item_dir(it).is_some_and(|d| d == node.path || d.starts_with(&prefix)))
                    .map(|it| it.modified)
                    .max();
                out.push(Entry {
                    kind: EntryKind::Folder,
                    name: node.name.clone(),
                    path: node.path.clone(),
                    modified,
                    size: 0,
                    ext: String::new(),
                    is_virtual: node.is_virtual,
                    item: None,
                });
            }
            for (i, it) in items.iter().enumerate().filter(|(_, it)| item_dir(it) == Some(dir.as_str())) {
                if keep(&it.name) {
                    out.push(video_entry(i, it, dir));
                }
            }
        }
    }
    out
}

fn video_entry(i: usize, it: &LibraryItem, dir: &str) -> Entry {
    Entry {
        kind: EntryKind::Video,
        name: it.name.clone(),
        path: dir.to_owned(),
        modified: Some(it.modified),
        size: it.size,
        ext: file_ext(&it.name),
        is_virtual: false,
        item: Some(i),
    }
}

fn find_children<'a>(tree: &'a Tree, dir: &str) -> &'a [DirNode] {
    if dir.is_empty() {
        return &tree.roots;
    }
    let mut level: &[DirNode] = &tree.roots;
    let mut acc = String::new();
    for seg in dir.split('/').filter(|s| !s.is_empty()) {
        acc = join_dir(&acc, seg);
        match level.iter().find(|n| n.path == acc) {
            Some(n) => level = &n.children,
            None => return &[],
        }
    }
    level
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Kind,
    Size,
}

/// Thư mục luôn trước (cả khi đảo chiều), rồi theo cột; hoà thì theo tên.
pub fn sort_entries(entries: &mut [Entry], key: SortKey, ascending: bool) {
    entries.sort_by(|a, b| {
        let folder_first = (a.kind != EntryKind::Folder).cmp(&(b.kind != EntryKind::Folder));
        if folder_first != Ordering::Equal {
            return folder_first;
        }
        let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
        let main = match key {
            SortKey::Name => by_name,
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Kind => a.ext.cmp(&b.ext),
            SortKey::Size => a.size.cmp(&b.size),
        };
        let main = if ascending { main } else { main.reverse() };
        main.then(by_name)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, dir: Option<&str>, size: u64, modified: i64) -> LibraryItem {
        LibraryItem {
            id: name.into(),
            name: name.into(),
            size,
            modified,
            mime: "video/mp4".into(),
            dir: dir.map(str::to_owned),
        }
    }

    fn sample() -> Vec<LibraryItem> {
        vec![
            item("root.mp4", Some(""), 5, 10),
            item("a.mp4", Some("Chuyến đi/Ngày 1"), 30, 50),
            item("b.mkv", Some("Chuyến đi/Ngày 1"), 20, 40),
            item("c.mp4", Some("Chuyến đi"), 10, 20),
            item("x.mp4", Some("Phim"), 1, 99),
            item("ext.mp4", None, 7, 5),
        ]
    }

    /// C-W4a — dir lồng ⇒ cây đủ tiền tố, `null` ⇒ Khác, thư mục ảo, sắp theo cột.
    #[test]
    fn tree_from_nested_dirs() {
        let t = build_tree(&sample(), &[]);
        assert!(t.has_other);
        let names: Vec<_> = t.roots.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["Chuyến đi", "Phim"]);
        let trip = &t.roots[0];
        assert_eq!(trip.children.len(), 1);
        assert_eq!(trip.children[0].path, "Chuyến đi/Ngày 1");
        assert!(!trip.children[0].is_virtual);
    }

    #[test]
    fn dir_prefixes_exist_without_own_files() {
        let t = build_tree(&[item("a.mp4", Some("x/y/z"), 1, 1)], &[]);
        assert_eq!(t.roots[0].path, "x");
        assert_eq!(t.roots[0].children[0].children[0].path, "x/y/z");
        assert!(!t.has_other);
    }

    #[test]
    fn virtual_dirs_are_marked_and_real_wins() {
        let t = build_tree(&sample(), &["Phim/Mới".into(), "Chuyến đi".into(), "Tách/Đôi".into()]);
        let trip = t.roots.iter().find(|n| n.name == "Chuyến đi").unwrap();
        assert!(!trip.is_virtual, "thư mục có tệp thật không còn ảo");
        let phim = t.roots.iter().find(|n| n.name == "Phim").unwrap();
        assert!(phim.children[0].is_virtual);
        let tach = t.roots.iter().find(|n| n.name == "Tách").unwrap();
        assert!(tach.is_virtual && tach.children[0].is_virtual);
    }

    #[test]
    fn entries_folders_first_and_sorted() {
        let items = sample();
        let mut e = list_entries(&items, &[], &Location::root(), "");
        sort_entries(&mut e, SortKey::Name, true);
        let names: Vec<_> = e.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["Chuyến đi", "Phim", "root.mp4"]);
        // Đảo chiều: thư mục vẫn trước.
        sort_entries(&mut e, SortKey::Name, false);
        assert_eq!(e[0].kind, EntryKind::Folder);
        assert_eq!(e[0].name, "Phim");
        // Ngày sửa của thư mục = mới nhất bên trong.
        let trip = list_entries(&items, &[], &Location::root(), "").into_iter().find(|x| x.name == "Chuyến đi").unwrap();
        assert_eq!(trip.modified, Some(50));
    }

    #[test]
    fn sort_by_size_kind_modified() {
        let items = sample();
        let at = Location::Dir("Chuyến đi/Ngày 1".into());
        let mut e = list_entries(&items, &[], &at, "");
        sort_entries(&mut e, SortKey::Size, true);
        assert_eq!(e[0].name, "b.mkv");
        sort_entries(&mut e, SortKey::Kind, true);
        assert_eq!(e[0].ext, "MKV");
        sort_entries(&mut e, SortKey::Modified, false);
        assert_eq!(e[0].name, "a.mp4");
    }

    #[test]
    fn other_group_and_filter() {
        let items = sample();
        let e = list_entries(&items, &[], &Location::Other, "");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "ext.mp4");
        assert!(!Location::Other.is_destination());
        let f = list_entries(&items, &[], &Location::Dir("Chuyến đi/Ngày 1".into()), "MKV");
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn virtual_folder_listed_in_parent() {
        let e = list_entries(&sample(), &["Phim/Mới".into()], &Location::Dir("Phim".into()), "");
        assert_eq!(e[0].name, "Mới");
        assert!(e[0].is_virtual);
        assert_eq!(e[1].name, "x.mp4");
    }

    #[test]
    fn path_helpers() {
        assert_eq!(parent_dir("a/b"), "a");
        assert_eq!(parent_dir("a"), "");
        assert_eq!(join_dir("", "a"), "a");
        assert_eq!(breadcrumb("a/b"), [("a".into(), "a".into()), ("b".into(), "a/b".into())]);
        assert_eq!(Location::Dir("a/b".into()).parent(), Some(Location::Dir("a".into())));
        assert_eq!(Location::root().parent(), None);
        assert_eq!(sanitize_folder_name("  x "), Some("x".into()));
        assert_eq!(sanitize_folder_name("a/b"), None);
        assert_eq!(sanitize_folder_name(".."), None);
        assert_eq!(file_ext("clip.MP4"), "MP4");
        assert_eq!(file_ext("noext"), "");
    }
}
