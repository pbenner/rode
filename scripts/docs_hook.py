"""Publish canonical repository references and images without duplicate sources."""

from pathlib import Path

from mkdocs.structure.files import File

ROOT = Path(__file__).resolve().parent.parent


def on_page_markdown(markdown, **_kwargs):
    # Source links work on GitHub; generated references use their site URLs.
    for source, destination in {
        "../themes/README.md": "theme-format.md",
        "../docs/syntax-themes.md": "syntax-themes.md",
        "../src/sandbox_policy.json": "https://github.com/pbenner/rode/blob/main/src/sandbox_policy.json",
    }.items():
        markdown = markdown.replace(f"]({source})", f"]({destination})")
    return markdown.replace("](../assets/", "](images/")


def on_files(files, config):
    files.append(
        File.generated(config, "theme-format.md", content=(ROOT / "themes/README.md").read_text())
    )
    for definition in sorted((ROOT / "themes").glob("*.json")):
        files.append(File.generated(config, definition.name, abs_src_path=str(definition)))
    for image in sorted((ROOT / "assets").glob("*.png")):
        files.append(File.generated(config, f"images/{image.name}", abs_src_path=str(image)))
    return files
