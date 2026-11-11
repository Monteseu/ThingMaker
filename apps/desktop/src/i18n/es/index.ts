/**
 * Spanish, split by area so each can be kept on its own. Spain's Spanish,
 * addressing the person as tú. See GLOSSARY.md for the terms used.
 */
import { bigthing } from "./bigthing";
import { local } from "./local";
import { logic } from "./logic";
import { session } from "./session";
import { settings } from "./settings";
import { shell } from "./shell";
import { ultrareview } from "./ultrareview";

export const es: Record<string, string> = { ...logic, ...session, ...settings, ...shell, ...bigthing, ...ultrareview, ...local };
