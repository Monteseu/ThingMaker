import type { AttachmentSnapshot } from "@thingmaker/contracts";
import { t } from "../i18n";
import { DocumentWarnings } from "./DocumentWarnings";

export function AttachmentChips({ files, onRemove }: { files: AttachmentSnapshot[]; onRemove: (id: string) => void }) {
  if (files.length === 0) return null;
  return (
    <div className="attachment-strip">
      {files.map((file) => (
        <span className={`attachment-chip${file.kind === "document" ? " attachment-chip-document" : ""}`} key={file.id}>
          <span className="small">{file.name}</span>
          <DocumentWarnings attachment={file} />
          <button className="link small" aria-label={t("remove attachment {name}", { name: file.name })} type="button" onClick={() => onRemove(file.id)}>×</button>
        </span>
      ))}
    </div>
  );
}
