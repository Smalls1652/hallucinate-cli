//! Read tool: bounded file and image reading.

use std::{
    fs::File,
    future::Future,
    io::{Read, Seek},
    pin::Pin,
};

use serde_json::json;

use super::{
    limits::{ToolLimits, parse_bounded_usize, run_blocking, truncate_utf8},
    paths::resolve_path,
    types::Tool,
};

/// File reading tool.
///
/// Reads text files and supported images without allocating a text file before
/// its size has been validated.
#[derive(Debug, Clone)]
pub struct ReadTool {
    limits: ToolLimits,

    /// Whether image files should be resized before attachment.
    auto_resize_images: bool,
}

impl Default for ReadTool {
    fn default() -> Self {
        Self {
            limits: ToolLimits::default(),
            auto_resize_images: true,
        }
    }
}

impl ReadTool {
    /// Create a new `ReadTool` with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure resource limits for this tool.
    pub fn with_limits(
        mut self,
        limits: ToolLimits,
    ) -> Self {
        self.limits = limits;
        self
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

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns text or an image attachment (jpg, png, gif, webp, bmp). Text and source images have bounded sizes. For large files, use grep or find to extract specific content."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to read" },
                "max_size": { "type": "integer", "description": "Maximum text file size in bytes (default: 1000000)" }
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
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required field: path"))?;
            
            let max_size = parse_bounded_usize(
                arguments,
                "max_size",
                self.limits.max_text_file_bytes,
                self.limits.max_text_file_bytes,
            )?;
            
            let path = resolve_path(path_str, cwd);
            let display_path = crate::config::paths::display_path(&path, cwd.unwrap_or("."));
            
            let limits = self.limits.clone();
            
            let auto_resize_images = self.auto_resize_images;

            run_blocking("read", move || {
                let mut file = match File::open(&path) {
                    Ok(file) => file,
            
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(super::types::ToolResult::error(
                            "read",
                            "read",
                            format!("File not found: {display_path}"),
                        ));
                    },
            
                    Err(error) => {
                        return Err(anyhow::Error::new(error)
                            .context(format!("failed to open {display_path}")));
                    },
                };
            
                let metadata = file.metadata().map_err(|error| {
                    anyhow::Error::new(error).context(format!("failed to inspect {display_path}"))
                })?;

                if metadata.is_dir() {
                    return Ok(super::types::ToolResult::error(
                        "read",
                        "read",
                        format!("Path is a directory, not a file: {display_path}"),
                    ));
                }

                let mut magic = [0_u8; 32];
                let magic_len = file.read(&mut magic)?;
                file.rewind()?;
            
                let is_image = crate::ai::images::is_supported_image(&magic[..magic_len]);
            
                let byte_limit = if is_image {
                    limits.max_image_source_bytes
                } else {
                    max_size
                };

                if metadata.len() > byte_limit as u64 {
                    let kind = if is_image { "Image file" } else { "File" };
            
                    return Ok(super::types::ToolResult::error(
                        "read",
                        "read",
                        format!(
                            "{kind} too large: {} bytes (max: {} bytes)",
                            metadata.len(),
                            byte_limit
                        ),
                    ));
                }

                let mut bytes =
                    Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(byte_limit));
            
                file.take(metadata.len().saturating_add(1))
                    .read_to_end(&mut bytes)?;
            
                if bytes.len() > byte_limit {
                    return Ok(super::types::ToolResult::error(
                        "read",
                        "read",
                        format!("File grew beyond its {byte_limit} byte limit while reading"),
                    ));
                }

                if is_image {
                    let image = crate::ai::images::process_image_bytes(&bytes, auto_resize_images)?;
            
                    return Ok(super::types::ToolResult::success(
                        "read",
                        "read",
                        format!("Read image file [{}]: {display_path}", image.mime_type),
                    )
                    .with_images(vec![image]));
                }

                let content = String::from_utf8(bytes).map_err(|_| {
                    anyhow::anyhow!("file is not valid UTF-8 text or a supported image")
                })?;
                let content = truncate_utf8(&content, max_size, "Content");

                Ok(super::types::ToolResult::success(
                    "read",
                    "read",
                    format!("File: {display_path}\n\n{content}"),
                ))
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;

    #[tokio::test]
    async fn rejects_an_oversized_file_before_reading_it() {
        let path = std::env::temp_dir().join(format!("hallucinate-read-{}", uuid::Uuid::new_v4()));
        let file = File::create(&path).unwrap();
        file.set_len(2_000_000).unwrap();

        let result = ReadTool::new()
            .execute(&json!({ "path": path }), None)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("too large"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn truncation_preserves_utf8_boundaries() {
        assert_eq!(
            truncate_utf8("aéz", 2, "Content"),
            "a\n\n[Content truncated: 4 total bytes]"
        );
    }

    #[tokio::test]
    async fn reads_images_as_attachments() {
        use image::{DynamicImage, ImageFormat};

        let path =
            std::env::temp_dir().join(format!("hallucinate-read-{}.png", uuid::Uuid::new_v4()));
        DynamicImage::new_rgba8(1, 1)
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();
        let result = ReadTool::new()
            .execute(&json!({ "path": path }), None)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.images[0].mime_type, "image/png");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn resolves_relative_paths() {
        assert_eq!(
            resolve_path("test.txt", Some("/some/dir")),
            Path::new("/some/dir/test.txt")
        );
    }
}
