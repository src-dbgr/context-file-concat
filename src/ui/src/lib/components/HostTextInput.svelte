<script lang="ts">
  /**
   * A text field for a value that the host owns.
   *
   * It shows the host's value, but the text being typed is local: snapshots
   * that arrive while the user is editing do not reset it (see `HostDraft`).
   * The edit is handed to `oncommit` when the field is left or Enter is
   * pressed (the `change` event); the caller sends it as an intent command.
   */
  import { untrack } from "svelte";
  import { HostDraft } from "$lib/modules/hostDraft";

  type Props = {
    id: string;
    /** The value the host reports. */
    value: string;
    oncommit: (value: string) => void;
  };
  let { id, value, oncommit }: Props = $props();

  const draft = new HostDraft<string>();
  let text = $state(untrack(() => value));

  $effect(() => {
    const host = value;
    untrack(() => {
      if (draft.accepts(host)) text = host;
    });
  });

  function commit() {
    draft.sent(text);
    oncommit(text);
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
