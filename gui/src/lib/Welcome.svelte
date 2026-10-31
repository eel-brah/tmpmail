<script lang="ts">
  // No inboxes yet: the first screen, showing what the app is for.
  import logo from "../assets/logo.svg";
  import CodeBanner from "./CodeBanner.svelte";
  import Icon from "./Icon.svelte";
  import { app } from "./store.svelte";
</script>

<section class="col-span-2 grid min-h-0 place-items-center overflow-y-auto border-l border-rule bg-raised p-10">
  <div class="w-full max-w-lg">
    <img src={logo} alt="" width="56" height="56" class="rounded-[14px]" />
    <h1 class="mt-6 text-[2rem] leading-[1.15] font-bold tracking-[-0.025em] text-balance">
      Throwaway addresses that catch your codes.
    </h1>
    <p class="mt-3 max-w-md text-sm text-muted">
      Make an address, sign up with it anywhere, and the verification code lands here, ready to copy.
    </p>
    <div class="mt-6 flex flex-wrap items-center gap-2">
      <button
        class="btn btn-primary !h-10 !px-4 !text-sm"
        disabled={app.creating}
        onclick={() => app.create({ provider: null, user: null, domain: null, label: null })}
      >
        <Icon name="plus" />{app.creating ? "Creating…" : "Make an address"}
      </button>
      <button class="btn !h-10 !px-4 !text-sm" onclick={() => (app.newInboxOpen = true)}>Choose a provider…</button>
    </div>
    <p class="mt-3 text-2xs text-muted">Or press <kbd>n</kbd> anytime, <kbd>N</kbd> to choose the provider.</p>
    <div class="mt-10" aria-hidden="true">
      <p class="mb-2 text-2xs font-semibold text-muted">A code, as it shows up</p>
      <CodeBanner code="482913" sample />
    </div>
  </div>
</section>
