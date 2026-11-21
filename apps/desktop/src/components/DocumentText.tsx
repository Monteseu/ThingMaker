import { useState } from "react";
import { t } from "../i18n";
import { ExtractionWarnings } from "./DocumentWarnings";

type Segment = { kind: "text"; text: string } | { kind: "document"; name: string; content: string; warnings: string[] };

/** Also recognizes persisted transcripts where a provider joined text blocks. */
export function documentSegments(text: string): Segment[] {
  const pattern = /^\[Attached document (.+) · [a-z]+\/[\w.+-]+ · blake3 [a-f0-9]{64}\]\r?\n([\s\S]*?)\[Document content begins\]\r?\n([\s\S]*?)\r?\n\[Document content ends\](?:\r?\n|$)/gm;
  const segments: Segment[] = [];
  let position = 0;
  for (const match of text.matchAll(pattern)) {
    if (match.index > position) segments.push({ kind: "text", text: text.slice(position, match.index) });
    segments.push({ kind: "document", name: match[1]!, content: match[3]!, warnings: match[2]!.split(/\r?\n/).filter((line) => line.startsWith("- ")).map((line) => line.slice(2)) });
    position = match.index + match[0].length;
  }
  if (position < text.length) segments.push({ kind: "text", text: text.slice(position) });
  return segments;
}

function DocumentPreview({ document }: { document: Extract<Segment, { kind: "document" }> }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="chat-document">
      <strong>{document.name}</strong>
      <ExtractionWarnings warnings={document.warnings} />
      <details onToggle={(event) => setOpen(event.currentTarget.open)}>
        <summary>{t("Preview extracted text")}</summary>
        {open && <pre className="text">{document.content}</pre>}
      </details>
    </div>
  );
}

export function DocumentText({ text }: { text: string }) {
  return <>{documentSegments(text).map((segment, index) => segment.kind === "document"
    ? <DocumentPreview document={segment} key={index} />
    : <pre className="text" key={index}>{segment.text}</pre>)}</>;
}
