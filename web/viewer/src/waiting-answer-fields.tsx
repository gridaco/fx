import { useId, useLayoutEffect, useRef } from "react";
import {
  answerKinds,
  answerSchema,
  emptyAnswer,
  encodeAnswer,
  initialAnswer,
  scalarEnumOptions,
  type AnswerDraft,
  type AnswerKind,
  type AnswerSchema,
} from "@grida/fx-web";

const labels: Record<AnswerKind, string> = {
  string: "Text", number: "Number", integer: "Whole number", boolean: "Yes or no",
  null: "No value", array: "List", object: "Fields",
};

interface EditorProps {
  label: string;
  schema: AnswerSchema;
  draft?: AnswerDraft;
  required?: boolean;
  onChange(draft: AnswerDraft | undefined): void;
}

function focusAnswerControl(container: Element | null) {
  container?.querySelector<HTMLElement>("input, select, button")?.focus();
}

function useAnswerRowFocus(rowCount: number | undefined) {
  const list = useRef<HTMLDivElement>(null);
  const pendingIndex = useRef<number | null>(null);
  useLayoutEffect(() => {
    const index = pendingIndex.current;
    if (index === null || !list.current) return;
    pendingIndex.current = null;
    const row = list.current.querySelectorAll(":scope > .fx-answer-row")[index];
    if (row) focusAnswerControl(row);
    else list.current.querySelector<HTMLElement>(":scope > .fx-answer-add")?.focus();
  }, [rowCount]);
  return { list, focusAfterChange: (index: number) => { pendingIndex.current = index; } };
}

function RequiredLabel({ title, required }: { title: string; required: boolean }) {
  return <>{title}{required && <span className="fx-answer-required" aria-hidden="true"> (required)</span>}</>;
}

function describeField(name: string) {
  const label = name.replaceAll("_", " ");
  return label ? label[0].toUpperCase() + label.slice(1) : "Unnamed field";
}

function ObjectFields({ schema, draft, onChange }: Omit<EditorProps, "label"> & {
  schema: Record<string, unknown>;
  draft: Extract<AnswerDraft, { kind: "object" }>;
}) {
  const properties = answerSchema(schema.properties);
  const names = typeof properties === "object" ? Object.keys(properties) : [];
  const patterns = answerSchema(schema.patternProperties);
  const hasPatterns = typeof patterns === "object" && Object.keys(patterns).length > 0;
  const allowCustom = schema.additionalProperties !== false || hasPatterns;
  // A user-entered name can match a pattern, in which case additionalProperties does not
  // apply. Keep every JSON type available and let the engine check the submitted name.
  const customSchema = hasPatterns ? true : answerSchema(schema.additionalProperties);
  const required = Array.isArray(schema.required) ? schema.required : [];
  const known = [...new Set([...names, ...required.filter((name): name is string => typeof name === "string")])];
  const replace = (index: number, field: (typeof draft.fields)[number]) =>
    onChange({ ...draft, fields: draft.fields.map((current, at) => at === index ? field : current) });
  const remove = (index: number) =>
    onChange({ ...draft, fields: draft.fields.filter((_, at) => at !== index) });
  const customFields = draft.fields.flatMap((field, index) => field.editableName ? [{ field, index }] : []);
  const { list, focusAfterChange } = useAnswerRowFocus(customFields.length);
  return <div className="fx-answer-fields" ref={list}>
    {known.map(name => {
      const index = draft.fields.findIndex(field => !field.editableName && field.name === name);
      return <div key={name}>
        <WaitingAnswerFields
          label={describeField(name)}
          required={required.includes(name)}
          schema={answerSchema(typeof properties === "object" && Object.hasOwn(properties, name) ? properties[name] : true)}
          draft={draft.fields[index]?.value}
          onChange={value => {
            if (value === undefined && index >= 0) remove(index);
            else if (value !== undefined && index < 0) onChange({ ...draft, fields: [...draft.fields, { name, value }] });
            else if (value !== undefined) replace(index, { name, value });
          }}
        />
        {index >= 0 && !required.includes(name) && <button type="button"
          className="fx-answer-helper" onClick={event => {
            const row = event.currentTarget.parentElement;
            remove(index);
            requestAnimationFrame(() => focusAnswerControl(row));
          }}>Clear {describeField(name)}</button>}
      </div>;
    })}
    {customFields.map(({ field, index }, row) => {
      return <div className="fx-answer-row" key={index}>
        <label>Field name<input value={field.name} onChange={event => replace(index, { ...field, name: event.currentTarget.value })} /></label>
        <WaitingAnswerFields label="Value" schema={customSchema}
          draft={field.value} required onChange={value => replace(index, { ...field, value })} />
        <button type="button" className="fx-answer-helper" onClick={() => {
          focusAfterChange(row);
          remove(index);
        }}>Remove field <span className="sr-only">{row + 1}</span></button>
      </div>;
    })}
    {allowCustom && <button type="button" className="fx-answer-helper fx-answer-add" onClick={() => {
      focusAfterChange(customFields.length);
      onChange({ ...draft, fields: [...draft.fields, { name: "", editableName: true, value: initialAnswer(customSchema) }] });
    }}>Add field</button>}
  </div>;
}

export function WaitingAnswerFields({ label, schema, draft, required = false, onChange }: EditorProps) {
  const id = useId();
  const { list, focusAfterChange } = useAnswerRowFocus(draft?.kind === "array" ? draft.items.length : undefined);
  if (schema === false) return <p>This field does not accept a value.</p>;
  const rule = schema === true ? {} : schema;
  const options = answerKinds(rule);
  const fixed = options.length === 1 ? options[0] : undefined;
  const current = draft ?? (fixed ? emptyAnswer(fixed, rule) : undefined);
  const title = typeof rule.title === "string" && rule.title.trim() ? rule.title : label;
  const description = typeof rule.description === "string" && rule.description.trim() ? rule.description : undefined;
  const describedBy = description ? `${id}-help` : undefined;
  const fieldLabel = <RequiredLabel title={title} required={required} />;
  const incomplete = current?.kind !== "object" && current?.kind !== "array"
    && (required || draft !== undefined) && encodeAnswer(draft) === undefined;
  const literals = scalarEnumOptions(rule);
  let control;
  if (literals) {
    const value = encodeAnswer(draft);
    control = <label className="fx-answer-text" htmlFor={id}><span>{fieldLabel}</span>
      <select id={id} value={value ?? ""} required={required} aria-describedby={describedBy}
        data-answer-error={required && value === undefined ? `Choose a value for ${title}.` : undefined}
        onChange={event => onChange(literals.find(option => option.encoded === event.currentTarget.value)?.draft)}>
        <option value="">Choose a value</option>
        {literals.map(option => <option key={option.encoded} value={option.encoded}>{option.label}</option>)}
      </select>
    </label>;
  } else if (!required && !draft && (fixed === "array" || fixed === "object")) {
    control = <fieldset aria-describedby={describedBy}><legend>{fieldLabel}</legend>
      <button type="button" className="fx-answer-helper" onClick={event => {
        const row = event.currentTarget.closest(".fx-answer-field")?.parentElement ?? null;
        onChange(emptyAnswer(fixed, rule));
        requestAnimationFrame(() => focusAnswerControl(row));
      }}>Include {title}</button>
    </fieldset>;
  } else if (current?.kind === "string" || current?.kind === "number" || current?.kind === "integer") {
    const unsigned = typeof rule.minimum === "number" && rule.minimum >= 0;
    const inputMode = !unsigned || current.kind === "string" ? "text" : current.kind === "integer" ? "numeric" : "decimal";
    const error = current.kind === "string" ? `Enter ${title}.`
      : current.kind === "integer" ? `Enter a whole number for ${title}.` : `Enter a number for ${title}.`;
    control = <label className="fx-answer-text" htmlFor={id}><span>{fieldLabel}</span><input id={id} value={current.text} inputMode={inputMode}
      aria-describedby={describedBy} aria-required={required || undefined} data-answer-error={incomplete ? error : undefined}
      onChange={event => onChange({ ...current, text: event.currentTarget.value })} /></label>;
  } else if (current?.kind === "boolean") {
    control = <fieldset className="fx-review-boolean" aria-describedby={describedBy}><legend>{fieldLabel}</legend>
      {[true, false].map(value => <label className="fx-answer-choice" key={String(value)}>
        <input type="radio" name={id} checked={current.value === value} required={required} aria-describedby={describedBy}
          data-answer-error={incomplete ? `Choose Yes or No for ${title}.` : undefined}
          onChange={() => onChange({ kind: "boolean", value })} />{value ? "Yes" : "No"}
      </label>)}
    </fieldset>;
  } else if (current?.kind === "null") {
    control = fixed && <fieldset aria-describedby={describedBy}><legend>{fieldLabel}</legend>
      <label className="fx-answer-choice"><input type="checkbox" checked={draft?.kind === "null"} required={required}
        aria-describedby={describedBy} data-answer-error={incomplete ? `Confirm no value for ${title}.` : undefined}
        onChange={event => onChange(event.currentTarget.checked ? current : undefined)} /> No value</label>
    </fieldset>;
  } else if (current?.kind === "object") {
    control = <fieldset aria-describedby={describedBy}><legend>{fieldLabel}</legend><ObjectFields schema={rule} draft={current} onChange={onChange} /></fieldset>;
  } else if (current?.kind === "array") {
    control = <fieldset aria-describedby={describedBy}><legend>{fieldLabel}</legend><div className="fx-answer-fields" ref={list}>
      {current.items.map((item, index) => {
        const itemSchema = answerSchema(Array.isArray(rule.prefixItems) && index < rule.prefixItems.length
          ? rule.prefixItems[index] : rule.items);
        return <div className="fx-answer-row" key={index}>
          <WaitingAnswerFields label={`Item ${index + 1}`} schema={itemSchema} draft={item} required
            onChange={value => onChange({ ...current, items: current.items.map((previous, at) => at === index ? value : previous) })} />
          <button type="button" className="fx-answer-helper"
            onClick={() => {
              focusAfterChange(index);
              onChange({ ...current, items: current.items.filter((_, at) => at !== index) });
            }}>Remove item {index + 1}</button>
        </div>;
      })}
      {(rule.items !== false || (Array.isArray(rule.prefixItems) && current.items.length < rule.prefixItems.length))
        && <button type="button" className="fx-answer-helper fx-answer-add" onClick={() => {
          const schema = answerSchema(Array.isArray(rule.prefixItems) && current.items.length < rule.prefixItems.length
            ? rule.prefixItems[current.items.length] : rule.items);
          focusAfterChange(current.items.length);
          onChange({ ...current, items: [...current.items, initialAnswer(schema)] });
        }}>Add item</button>}
    </div></fieldset>;
  }
  return <div className="fx-answer-field">
    {description && <p id={`${id}-help`} className="fx-review-meta">{description}</p>}
    {!fixed && !literals && <label className="fx-answer-text"><span><RequiredLabel title={`${title} type`} required={required} /></span>
      <select value={draft?.kind ?? ""} required={required} aria-describedby={describedBy}
        data-answer-error={required && !draft ? `Choose a type for ${title}.` : undefined} onChange={event => {
      const kind = options.find(option => option === event.currentTarget.value);
      onChange(kind ? emptyAnswer(kind, rule) : undefined);
    }}>
      <option value="">Choose a type</option>
      {options.map(option => <option key={option} value={option}>{labels[option]}</option>)}
    </select></label>}
    {control}
  </div>;
}
