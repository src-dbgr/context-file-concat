<script lang="ts">
  /**
   * A text field for a value that the host owns.
   *
   * It shows the host's value, but the text being typed is local: snapshots
   * that arrive while the user is editing do not reset it, and neither do
   * those from before the host has processed the edit (see `HostDraft`).
   * The edit is handed to `oncommit` when the field is left or Enter is
   * pressed (the `change` event); the caller sends it as an intent command
   * and returns the name of that command (what `post()` returns).
   */
  import { untrack } from "svelte";
  import { HostDraft } from "$lib/modules/hostDraft";
  import { appState } from "$lib/stores/app";
  import type { CommandId } from "$lib/ipc/schema";

  type Props = {
    id: string;
    /** The value the host reports. */
    value: string;
    /** Sends the edit; returns what `post()` returned for its command. */
    oncommit: (value: string) => CommandId | null;
  };
  let { id, value, oncommit }: Props = $props();

  const draft = new HostDraft();
  let text = $state(untrack(() => value));

  // Runs for every snapshot of the host: shows the host's value unless the
  // draft holds it back.
  $effect(() => {
    const snapshot = $appState;
    void value;
    untrack(() => {
      if (draft.accepts(snapshot)) text = value;
    });
  });

  function commit() {
    draft.sent(oncommit(text));
  }

  // Left without a `change` event: the text is what it was when the field was
  // entered, so there is nothing to send and the host's value applies again.
  function onBlur() {
    if (!draft.isEditing) return;
    draft.reset();
    text = value;
  }
</script>

<input
  type="text"
  {id}
  bind:value={text}
  oninput={() => draft.edit()}
  onchange={commit}
  onblur={onBlur}
/>
