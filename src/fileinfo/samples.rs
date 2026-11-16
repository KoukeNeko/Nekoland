//! Made-up files of every kind, as FinderHover's preview samples: the settings show the panel
//! with them, and list from them what each kind's block can leave out. Kinds read from text
//! are read by their own readers, so what they list cannot drift from what the panel shows.

use super::*;

/// The samples, each with its content type; the first is a plain file, with every detail a
/// file can have.
pub fn samples() -> Vec<(Facts, &'static str)> {
    let mut plain = sample("會議紀錄.txt", "text/plain", 4_812, None);
    plain.0.tags = vec!["工作".into(), "重要".into()];
    plain.0.source = block(
        "source",
        PURPLE,
        vec![
            Some(Part::Source {
                host: "example.com".into(),
                rest: "/notes/2026-09.txt".into(),
                address: "https://example.com/notes/2026-09.txt".into(),
            }),
            fields(rows(&[("參照頁面", "https://example.com/notes", true)]), 2),
        ],
    );
    plain.0.file = rows(&[
        ("實際佔用", "8.2 kB", false),
        ("類型識別碼", "text/plain", true),
        ("連結目標", "~/文件/會議紀錄.txt", true),
        ("硬連結數", "2", false),
        ("屬性修改日期", "2026/9/3 09:40", false),
        ("擴展屬性", "3", false),
        ("模式", "rw-r--r-- (644)", false),
        ("擁有者", "neko:neko", false),
        ("隱藏檔案", "否", false),
    ]);
    plain.0.note = Ok("週五前回覆".into());

    let mut xcode = sample(
        "Nekoland.xcodeproj",
        "inode/directory",
        0,
        block(
            "Xcode 專案",
            BLUE,
            vec![fields(
                rows(&[
                    ("Target 數", "3", false),
                    ("Swift 版本", "6.0", false),
                    ("部署目標", "macOS 15.0", false),
                    ("配置數", "6", false),
                    ("組織名稱", "Nekoland", false),
                    ("單元測試", "是", false),
                    ("UI 測試", "是", false),
                ]),
                2,
            )],
        ),
    );
    xcode.0.details = format!(
        "{} · 3 個項目",
        gio::content_type_get_description("inode/directory")
    );
    xcode.0.size = None;

    let mut git = sample("Nekoland", "inode/directory", 0, None);
    git.0.details = format!(
        "{} · 24 個項目",
        gio::content_type_get_description("inode/directory")
    );
    git.0.size = None;
    git.0.block = block(
        "Git",
        ORANGE,
        vec![
            Some(Part::Hero {
                value: Some("main".into()),
                unit: None,
                trailing: Some("有未提交的變更".into()),
                small: true,
            }),
            Some(Part::Fields(
                rows(&[("最後提交 · 2026/9/3 08:10", "feat: show what a file holds", true)]),
                1,
            )),
            specs(vec![
                plain_spec("1,284", "提交數"),
                plain_spec("6", "分支數"),
                plain_spec("12", "標籤數"),
            ]),
            fields(rows(&[("遠端", "git@example.com:neko/nekoland.git", true)]), 1),
        ],
    );

    let mut photo = sample(
        "DSC03757.jpg",
        "image/jpeg",
        21_600_000,
        block(
            "照片",
            YELLOW,
            vec![
                title(
                    Some("SONY ILCE-7C".into()),
                    13.0,
                    None,
                    Some("28-70mm F2.8 DG DN | Contemporary 021".into()),
                ),
                specs(vec![
                    colored_spec("28 mm", "焦距"),
                    colored_spec("ƒ/4.0", "光圈"),
                    colored_spec("1/15 s", "快門"),
                    colored_spec("500", "ISO"),
                ]),
                fields(
                    rows(&[
                        ("拍攝", "2026/9/2 21:33", false),
                        ("位置", "37.7048°N, 138.8276°E", false),
                        ("創作工具", "ILCE-7C v2.00", false),
                        ("色彩描述檔", "Display P3", false),
                        ("色彩空間", "RGB", false),
                        ("位元深度", "14-bit", false),
                        ("HDR 增益圖", "是", false),
                        ("標題", "新潟的夜晚", true),
                        ("創作者", "neko", false),
                        ("版權", "© 2026 neko", false),
                        ("評分", "★★★★☆", false),
                        ("關鍵字", "新潟, 夜景", true),
                        ("描述", "信濃川畔的煙火", true),
                    ]),
                    2,
                ),
            ],
        ),
    );
    photo.0.frame = Some((3936, 2624));

    let markdown = text_sample(
        "README.md",
        "text/markdown",
        markdown_block(
            "---\ntitle: 旅行計畫\n---\n# 新潟\n\n第一天到 [新潟站](https://example.com)，\
             晚上看煙火。\n\n![煙火](fireworks.jpg)\n\n```\nshinkansen --to niigata\n```\n",
            "UTF-8",
        ),
    );
    let html = text_sample(
        "index.html",
        "text/html",
        html_block(
            r#"<html lang="zh-TW"><head><meta charset="UTF-8"><title>Nekoland</title>
<meta name="description" content="A Wayland compositor that feels like a Mac.">
<meta name="keywords" content="wayland, compositor"><meta name="author" content="neko">
<meta property="og:title" content="Nekoland"><meta property="og:description" content="A Mac-like desktop.">
<meta property="og:image" content="https://example.com/cover.png"><meta name="twitter:card" content="summary_large_image">
</head></html>"#,
            "UTF-8",
        ),
    );
    let subtitle = text_sample(
        "新潟花火.vtt",
        "text/vtt",
        subtitle_block(
            "WEBVTT\nLanguage: zh-TW\n\n00:00:01.000 --> 00:00:04.000\n<i>煙火開始了</i>\n\n\
             12:44.000 --> 12:48.000\n明年見\n",
            "UTF-8",
            "vtt",
        ),
    );
    let config = text_sample(
        "config.toml",
        "application/toml",
        config_block(
            "# 外觀\n[dock]\nautohide = true\nmagnify = 1.6\n\n[hover]\ndelay = 0.1\n",
            "ASCII",
            "toml",
        ),
    );
    let code = text_sample(
        "main.rs",
        "text/rust",
        language(Path::new("main.rs")).and_then(|rust| {
            code_block(
                "// The compositor.\nfn main() {\n    let state = Nekoland::new();\n\n    state.run();\n}\n",
                "UTF-8",
                &rust,
            )
        }),
    );

    vec![
        plain,
        photo,
        sample(
            "新潟花火.mov",
            "video/quicktime",
            1_240_000_000,
            block(
                "影片",
                INDIGO,
                vec![
                    Some(Part::Hero {
                        value: Some("12:48".into()),
                        unit: Some("時長".into()),
                        trailing: Some("HLG".into()),
                        small: false,
                    }),
                    specs(vec![
                        plain_spec("3840×2160", "解析度"),
                        plain_spec("59.94 fps", "影格率"),
                        plain_spec("HEVC", "編碼"),
                        plain_spec("48.2 Mbps", "位元率"),
                    ]),
                    Some(Part::Caption(
                        "QuickTime · 1 個視訊軌 · 1 個音訊軌 · 2 個章節".into(),
                    )),
                    fields(
                        rows(&[("色域", "BT.2020", false), ("轉換函數", "ARIB-STD-B67", false)]),
                        2,
                    ),
                ],
            ),
        ),
        sample(
            "03 月光.mp3",
            "audio/mpeg",
            11_900_000,
            block(
                "音訊",
                PINK,
                vec![
                    title(
                        Some("月光".into()),
                        16.0,
                        Some("德布西".into()),
                        Some("貝加馬斯克組曲 · 1905 · 古典".into()),
                    ),
                    specs(vec![
                        plain_spec("5:12", "時長"),
                        plain_spec("320 kbps", "位元率"),
                        plain_spec("44.1 kHz", "取樣率"),
                        plain_spec("立體聲", "聲道"),
                    ]),
                    fields(rows(&[("專輯演出者", "德布西", false)]), 2),
                ],
            ),
        ),
        sample(
            "attention-is-all-you-need.pdf",
            "application/pdf",
            2_200_000,
            block(
                "PDF",
                RED,
                vec![
                    title(
                        Some("Attention Is All You Need".into()),
                        15.0,
                        None,
                        Some("Ashish Vaswani 等".into()),
                    ),
                    specs(specs_first_colored(vec![
                        spec(Some("15".into()), "頁數", false),
                        spec(Some("Letter".into()), "頁面大小", false),
                        spec(Some("1.5".into()), "PDF 版本", false),
                        spec(Some("否".into()), "已加密", false),
                    ])),
                    fields(
                        rows(&[
                            ("主旨", "Neural machine translation", true),
                            ("關鍵字", "transformer, attention", true),
                            ("建立者", "LaTeX", false),
                            ("製作者", "pdfTeX-1.40.25", false),
                            ("文件建立", "2023/8/3 00:07", false),
                            ("文件修改", "2023/8/3 00:07", false),
                        ]),
                        2,
                    ),
                ],
            ),
        ),
        sample(
            "季報.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            86_000,
            block(
                "Office 文件",
                BLUE,
                vec![
                    title(Some("第三季營運報告".into()), 15.0, None, Some("neko".into())),
                    specs(specs_first_colored(vec![
                        spec(Some("12".into()), "頁數", false),
                        spec(Some("4,318".into()), "字數", false),
                        spec(Some("3".into()), "工作表數", false),
                        spec(Some("0".into()), "投影片數", false),
                    ])),
                    fields(
                        rows(&[
                            ("主旨", "營運", true),
                            ("關鍵字", "季報, 2026", true),
                            ("註解", "給董事會的版本", true),
                            ("文件建立", "2026/9/1 10:00", false),
                            ("文件修改", "2026/9/3 08:10", false),
                            ("修改者", "neko", false),
                            ("公司", "Nekoland", false),
                            ("類別", "報告", false),
                        ]),
                        2,
                    ),
                ],
            ),
        ),
        sample(
            "相片.zip",
            "application/zip",
            184_000_000,
            block(
                "壓縮檔",
                BROWN,
                vec![
                    hero(Some("1,206".into()), Some("個檔案".into()), Some("已加密".into())),
                    Some(Part::Sizes(vec![
                        ("未壓縮".into(), 215_000_000, Shade::Fill),
                        ("壓縮檔".into(), 184_000_000, Shade::Accent),
                    ])),
                    fields(rows(&[("格式", "ZIP", false), ("壓縮率", "14%", false)]), 3),
                ],
            ),
        ),
        sample(
            "雪國.epub",
            "application/epub+zip",
            1_400_000,
            block(
                "電子書",
                BROWN,
                vec![fields(
                    rows(&[
                        ("書名", "雪國", true),
                        ("作者", "川端康成", false),
                        ("出版社", "木馬文化", false),
                        ("出版日期", "2019/1/1 00:00", false),
                        ("ISBN", "9789863595987", false),
                        ("語言", "zh-TW", false),
                        ("內容簡介", "穿過縣界長長的隧道，便是雪國。", true),
                    ]),
                    2,
                )],
            ),
        ),
        code,
        sample(
            "Ubuntu-R.ttf",
            "font/ttf",
            1_100_000,
            block(
                "字型",
                GREEN,
                vec![
                    title(Some("Ubuntu".into()), 15.0, Some("Regular".into()), None),
                    fields(
                        rows(&[
                            ("名稱", "Ubuntu Regular", false),
                            ("版本", "Version 0.869", false),
                            ("字形數", "1,843", false),
                            ("設計者", "Dalton Maag Ltd", false),
                            ("版權", "Copyright 2011 Canonical Ltd.", true),
                        ]),
                        2,
                    ),
                ],
            ),
        ),
        sample(
            "ubuntu-26.10-desktop-amd64.iso",
            "application/x-cd-image",
            6_100_000_000,
            block(
                "磁碟映像",
                CYAN,
                vec![
                    specs(vec![
                        plain_spec("ISO 9660", "格式"),
                        plain_spec("MBR", "分割區配置"),
                    ]),
                    Some(Part::Sizes(vec![("總大小".into(), 6_100_000_000, Shade::Fill)])),
                    fields(
                        rows(&[
                            ("磁碟區名稱", "Ubuntu 26.10 amd64", false),
                            ("可開機", "是", false),
                            ("加密狀態", "否", false),
                        ]),
                        3,
                    ),
                ],
            ),
        ),
        sample(
            "logo.svg",
            "image/svg+xml",
            12_400,
            block(
                "向量圖形",
                ORANGE,
                vec![fields(
                    rows(&[
                        ("格式", "SVG", false),
                        ("尺寸", "512 × 512", false),
                        ("視圖框", "0 0 512 512", false),
                        ("元素數", "48", false),
                        ("建立者", "Inkscape 1.4", false),
                        ("版本", "1.1", false),
                    ]),
                    2,
                )],
            ),
        ),
        subtitle,
        html,
        markdown,
        config,
        sample(
            "海報.psd",
            "image/vnd.adobe.photoshop",
            96_000_000,
            block(
                "PSD",
                BLUE,
                vec![fields(
                    rows(&[
                        ("尺寸", "4961 × 7016", false),
                        ("圖層數", "24", false),
                        ("色彩模式", "CMYK", false),
                        ("位元深度", "8 bit", false),
                        ("透明度", "是", false),
                    ]),
                    2,
                )],
            ),
        ),
        sample(
            "nekoland",
            "application/x-executable",
            11_500_000,
            block(
                "執行檔",
                BLUE,
                vec![fields(
                    rows(&[
                        ("架構", "x86-64", false),
                        ("檔案類型", "64-bit LSB pie executable", false),
                        ("連結", "動態", false),
                        ("最低核心", "Linux 3.2.0", false),
                        ("符號", "已移除", false),
                        ("載入器", "/lib64/ld-linux-x86-64.so.2", true),
                        ("BuildID", "fa97f0279523f3ae8d365f18fd3be7da8cc8a745", true),
                    ]),
                    2,
                )],
            ),
        ),
        sample(
            "nekoland.desktop",
            "application/x-desktop",
            412,
            block(
                "應用程式",
                BLUE,
                vec![
                    title(Some("Nekoland".into()), 15.0, None, Some("Mac 風格的桌面".into())),
                    fields(
                        rows(&[
                            ("識別碼", "nekoland", true),
                            ("指令", "nekoland --tty-udev", true),
                            ("類別", "System", false),
                            ("版本", "1.5", false),
                        ]),
                        2,
                    ),
                ],
            ),
        ),
        sample(
            "nekoland_0.1.0_amd64.deb",
            "application/vnd.debian.binary-package",
            4_200_000,
            block(
                "Debian 套件",
                BLUE,
                vec![
                    hero(Some("0.1.0".into()), None, Some("x11".into())),
                    fields(
                        rows(&[
                            ("套件", "nekoland", true),
                            ("架構", "amd64", false),
                            ("安裝大小", "14.2 MB", false),
                            ("維護者", "neko <neko@example.com>", true),
                            ("描述", "A Mac-like Wayland compositor", true),
                        ]),
                        2,
                    ),
                ],
            ),
        ),
        sample(
            "library.sqlite",
            "application/vnd.sqlite3",
            12_300_000,
            block(
                "SQLite",
                BLUE,
                vec![fields(
                    rows(&[
                        ("資料表數", "8", false),
                        ("索引數", "12", false),
                        ("觸發器數", "2", false),
                        ("視圖數", "1", false),
                        ("總列數", "48,210", false),
                        ("架構版本", "14", false),
                        ("頁面大小", "4096 bytes", false),
                        ("編碼", "UTF-8", false),
                    ]),
                    2,
                )],
            ),
        ),
        sample(
            "貓.glb",
            "model/gltf-binary",
            3_800_000,
            block(
                "3D 模型",
                ORANGE,
                vec![fields(
                    rows(&[
                        ("格式", "glTF (二進位)", false),
                        ("頂點數", "24,816", false),
                        ("面數", "41,302", false),
                        ("網格數", "6", false),
                        ("材質數", "4", false),
                        ("動畫數", "3", false),
                        ("骨架", "是", false),
                        ("邊界框", "1.20 × 0.84 × 2.31", true),
                    ]),
                    2,
                )],
            ),
        ),
        xcode,
        git,
    ]
}

/// What each part of the panel can leave out, a line each: the part's name, then its fields'
/// labels, by tabs. The dates and file details come first, then every kind.
pub fn catalogue() -> String {
    let samples = samples();
    let labels = |fields: &[Field]| {
        fields
            .iter()
            .map(|field| plain_label(&field.label).to_string())
            .collect()
    };
    let mut lines: Vec<(String, Vec<String>)> = vec![
        ("dates".into(), labels(&samples[0].0.dates)),
        ("file".into(), labels(&samples[0].0.file)),
    ];
    for (facts, _) in &samples {
        let Some(block) = &facts.block else { continue };
        let mut names: Vec<String> = Vec::new();
        for part in &block.parts {
            let found: Vec<String> = match part {
                Part::Specs(specs) => specs.iter().map(|spec| spec.label.clone()).collect(),
                Part::Fields(fields, _) => labels(fields),
                _ => Vec::new(),
            };
            names.extend(
                found
                    .into_iter()
                    .filter(|name| !names.contains(name))
                    .collect::<Vec<_>>(),
            );
        }
        lines.push((block.name.to_string(), names));
    }
    lines
        .into_iter()
        .map(|(name, labels)| format!("{name}\t{}\n", labels.join("\t")))
        .collect()
}

/// A label as `hover_hide` names it: git's last commit carries its date after a dot.
pub fn plain_label(label: &str) -> &str {
    label.split(" · ").next().unwrap_or(label)
}

fn sample(name: &str, kind: &'static str, bytes: u64, block: Option<Block>) -> (Facts, &'static str) {
    let description = gio::content_type_get_description(kind).to_string();
    let size = size_text(bytes);
    let facts = Facts {
        path: PathBuf::from(name),
        name: name.into(),
        details: format!("{description} · {size}"),
        kind: description,
        size: Some(size.clone()),
        tags: Vec::new(),
        block,
        source: None,
        frame: None,
        dates: rows(&[
            ("建立", "2026/9/2 21:33", false),
            ("修改", "2026/9/3 08:10", false),
            ("上次開啟", "今天 09:12", true),
        ]),
        modified: Some("2026/9/3 08:10".into()),
        file: rows(&[
            ("實際佔用", &size, false),
            ("類型識別碼", kind, true),
            ("模式", "rw-r--r-- (644)", false),
            ("擁有者", "neko:neko", false),
        ]),
        folder: "~ › 文件".into(),
        volume: "Nekoland".into(),
        available: Some("可用 412 GB，共 1 TB".into()),
        used: Some(0.6),
        note: Ok(String::new()),
    };
    (facts, kind)
}

fn text_sample(name: &str, kind: &'static str, block: Option<Block>) -> (Facts, &'static str) {
    sample(name, kind, 2_048, block)
}

fn rows(list: &[(&str, &str, bool)]) -> Vec<Field> {
    list.iter()
        .map(|(label, value, wide)| Field {
            label: (*label).into(),
            value: (*value).into(),
            wide: *wide,
        })
        .collect()
}

fn plain_spec(value: &str, label: &str) -> Spec {
    Spec {
        value: value.into(),
        label: label.into(),
        colored: false,
    }
}

fn colored_spec(value: &str, label: &str) -> Spec {
    Spec {
        colored: true,
        ..plain_spec(value, label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_sample_with_fields_to_leave_out() {
        let catalogue = catalogue();
        let names: Vec<&str> = catalogue
            .lines()
            .map(|line| line.split('\t').next().unwrap())
            .collect();
        assert_eq!(&names[..2], ["dates", "file"]);
        assert_eq!(names.len(), 2 + 23);
        // Kinds read from text list what their own readers show.
        let markdown = catalogue
            .lines()
            .find(|line| line.starts_with("Markdown\t"))
            .unwrap();
        assert_eq!(markdown, "Markdown\t字數\t標題數\t連結數\t圖片數\t程式碼區塊");
        assert!(catalogue.lines().all(|line| line.split('\t').count() > 1));
    }
}
