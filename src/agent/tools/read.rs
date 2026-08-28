//! Read tool: read file contents.

use std::{future::Future, path::Path, pin::Pin};

use serde_json::json;

use super::types::Tool;

/// File reading tool.
///
/// Reads file contents with configurable size limits.
#[derive(Debug, Clone)]
pub struct ReadTool {
    /// Maximum text file size in bytes.
    max_size: usize,
    /// Whether image files should be resized before attachment.
    auto_resize_images: bool,
}

impl Default for ReadTool {
    fn default() -> Self {
        Self {
            max_size: 1_000_000,
            auto_resize_images: true,
        } // 1MB default
    }
}

impl ReadTool {
    /// Create a new ReadTool with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure whether returned image attachments are automatically resized.
    pub fn with_auto_resize_images(
        mut self,
        enabled: bool,
    ) -> Self {
        self.auto_resize_images = enabled;
        self
    }
}

/// Resolve a file path, handling tilde expansion and relative paths.
pub(crate) fn resolve_path(
    path_str: &str,
    cwd: Option<&str>,
) -> std::path::PathBuf {
    let expanded = shellexpand::full(path_str).unwrap_or(std::borrow::Cow::Borrowed(path_str));
    let path = Path::new(expanded.as_ref());
    if path.is_relative() {
        if let Some(cwd) = cwd {
            return Path::new(cwd).join(path);
        }
        return std::env::current_dir().unwrap_or_default().join(path);
    }
    path.to_path_buf()
}

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns the file content as a string. Supports text files and images (jpg, png, gif, webp, bmp). For large files, consider using grep or find to extract specific content."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "max_size": {
                    "type": "integer",
                    "description": "Maximum file size in bytes (default: 1000000)"
                }
            },
            "required": ["path"]
        })
    }

    fn execute<'a>(
        &'a self,
        arguments: &'a serde_json::Value,
        cwd: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<super::types::ToolResult, anyhow::Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let path_str = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: path"))?;

            let max_size = arguments
                .get("max_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.max_size as u64) as usize;

            let path = resolve_path(path_str, cwd);
            let display_path = crate::config::paths::display_path(&path, cwd.unwrap_or("."));

            if !path.exists() {
                return Ok(super::types::ToolResult::error(
                    "read",
                    "read",
                    format!("File not found: {display_path}"),
                ));
            }

            if path.is_dir() {
                return Ok(super::types::ToolResult::error(
                    "read",
                    "read",
                    format!("Path is a directory, not a file: {display_path}"),
                ));
            }

            let metadata = std::fs::metadata(&path)?;
            let bytes = std::fs::read(&path)?;
            if crate::ai::images::is_supported_image(&bytes) {
                // Allow source images larger than the text limit so automatic
                // resizing can make them safe to attach to a vision model.
                const MAX_SOURCE_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
                if metadata.len() > MAX_SOURCE_IMAGE_BYTES {
                    return Ok(super::types::ToolResult::error(
                        "read",
                        "read",
                        format!(
                            "Image file too large: {} bytes (max source size: {} bytes)",
                            metadata.len(),
                            MAX_SOURCE_IMAGE_BYTES
                        ),
                    ));
                }
                let image =
                    crate::ai::images::process_image_bytes(&bytes, self.auto_resize_images)?;
                return Ok(super::types::ToolResult::success(
                    "read",
                    "read",
                    format!("Read image file [{}]: {display_path}", image.mime_type),
                )
                .with_images(vec![image]));
            }
            if metadata.len() as usize > max_size {
                return Ok(super::types::ToolResult::error(
                    "read",
                    "read",
                    format!(
                        "File too large: {} bytes (max: {} bytes)",
                        metadata.len(),
                        max_size
                    ),
                ));
            }

            let content = String::from_utf8(bytes).map_err(|_| {
                anyhow::anyhow!("file is not valid UTF-8 text or a supported image")
            })?;
            let truncated = truncate_output(&content, max_size);
            Ok(super::types::ToolResult::success(
                "read",
                "read",
                format!("File: {display_path}\n\n{truncated}"),
            ))
        })
    }
}

fn truncate_output(
    content: &str,
    max_size: usize,
) -> String {
    if content.len() <= max_size {
        content.to_string()
    } else {
        let truncated = &content[..max_size];
        format!(
            "{}\n\n[Content truncated: {} total characters]",
            truncated,
            content.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_tool_exists() {
        let tool = ReadTool::new();
        assert_eq!(tool.name(), "read");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let tool = ReadTool::new();
        let args = json!({ "path": "/nonexistent/file.txt" });
        let result = futures::executor::block_on(tool.execute(&args, None));
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_read_missing_path() {
        let tool = ReadTool::new();
        let args = json!({});
        let result = futures::executor::block_on(tool.execute(&args, None));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_image_returns_base64_attachment() {
        use image::{DynamicImage, ImageFormat};

        let path =
            std::env::temp_dir().join(format!("hallucinate-read-{}.png", uuid::Uuid::new_v4()));
        DynamicImage::new_rgba8(1, 1)
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();

        let tool = ReadTool::new();
        let args = json!({ "path": path });
        let result = tool.execute(&args, None).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].mime_type, "image/png");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_resolve_path_relative() {
        let path = resolve_path("test.txt", Some("/some/dir"));
        assert_eq!(path, Path::new("/some/dir/test.txt"));
    }

    #[test]
    fn test_resolve_path_absolute() {
        let path = resolve_path("/absolute/path.txt", Some("/some/dir"));
        assert_eq!(path, Path::new("/absolute/path.txt"));
    }
}
