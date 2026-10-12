<script lang="ts">
	import type { LayoutProps } from './$types';

	let { data, children }: LayoutProps = $props();
</script>

<header class="top">
	<span class="brand">Auau</span>
	{#if data.user}
		<span class="who">{data.user.name}</span>
	{/if}
</header>

<main>
	{@render children()}
</main>

<style>
	.top {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		padding: 0.75rem 1rem;
		border-bottom: 1px solid #ddd;
	}

	.brand {
		font-weight: 700;
	}

	.who {
		color: #555;
		font-size: 0.9rem;
	}

	main {
		max-width: 64rem;
		margin: 0 auto;
		padding: 1rem;
	}
</style>
