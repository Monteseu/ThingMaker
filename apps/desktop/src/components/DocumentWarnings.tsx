import type { AttachmentSnapshot } from "@thingmaker/contracts";
import { t } from "../i18n";

export function DocumentWarnings({ attachment }: { attachment: AttachmentSnapshot }) {
  const warnings = attachment.document?.warnings ?? [];
  if (attachment.kind !== "document" || warnings.length === 0) return null;
  return <ExtractionWarnings warnings={warnings} />;
}

export function ExtractionWarnings({ warnings }: { warnings: string[] }) {
  if (warnings.length === 0) return null;
  return (
    <span className="document-warnings">
      {warnings.map((warning, index) => (
        <span className="small chip-warn" key={index}>
          {t("Extraction warning")}: {warning}
        </span>
      ))}
    </span>
  );
}
