use proc_macro::TokenStream;
use quote::quote;
use std::path::Path;
use walkdir::WalkDir;

/// 嵌入指定目录的所有文件到可执行文件中
///
/// # 用法
/// ```rust
/// embed_dir::embed_dir!("assets");
/// ```
///
/// 这会在构建时将 `assets` 目录打包进可执行文件，
/// 运行时如果 `assets` 目录不存在则自动解压。
#[proc_macro]
pub fn embed_dir(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();
    let dir_path = input_str.trim().trim_matches('"');

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set");
    let full_path = Path::new(&manifest_dir).join(dir_path);

    if !full_path.exists() {
        panic!("Directory not found: {}", full_path.display());
    }

    let mut file_entries = Vec::new();

    for entry in WalkDir::new(&full_path) {
        let entry = entry.expect("Failed to read directory entry");
        if entry.file_type().is_file() {
            let file_path = entry.path();
            let relative_path = file_path
                .strip_prefix(&full_path)
                .expect("Failed to get relative path")
                .to_string_lossy()
                .replace('\\', "/");

            let absolute_path = file_path.to_string_lossy().to_string();

            file_entries.push((relative_path, absolute_path));
        }
    }

    let relative_paths: Vec<_> = file_entries.iter().map(|(r, _)| r.as_str()).collect();
    let absolute_paths: Vec<_> = file_entries.iter().map(|(_, a)| a.as_str()).collect();
    let dir_path_str = dir_path.to_string();

    let expanded = quote! {
        {
            const EMBEDDED_FILES: &[(&str, &[u8])] = &[
                #(
                    (#relative_paths, include_bytes!(#absolute_paths))
                ),*
            ];
            
            fn extract_embedded_dir() -> std::io::Result<bool> {
                use std::fs;
                use std::path::Path;
                
                let target_dir = Path::new(#dir_path_str);
                
                if target_dir.exists() {
                    return Ok(false);
                }
                
                for (relative_path, content) in EMBEDDED_FILES.iter() {
                    let file_path = target_dir.join(relative_path);
                    
                    if let Some(parent) = file_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    
                    fs::write(&file_path, content)?;
                }
                
                Ok(true)
            }
            
            extract_embedded_dir()
        }
    };

    expanded.into()
}