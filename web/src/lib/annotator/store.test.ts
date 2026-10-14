import { describe, expect, it } from 'vitest';
import { AnnotationState } from './store';

describe('AnnotationState', () => {
	it('initial_state_is_empty_without_selection', () => {
		const state = new AnnotationState();
		expect(state.segments).toEqual([]);
		expect(state.selectedLabelId).toBeNull();
	});

	it('selectLabel_sets_selected_label', () => {
		const state = new AnnotationState();
		state.selectLabel('label-1');
		expect(state.selectedLabelId).toBe('label-1');
		state.selectLabel('label-2');
		expect(state.selectedLabelId).toBe('label-2');
	});

	it('addSegment_appends_segment_and_returns_it', () => {
		const state = new AnnotationState();
		const a = state.addSegment('label-1', 0, 100);
		const b = state.addSegment('label-2', 100, 250);
		expect(a).toMatchObject({ label_id: 'label-1', start_ms: 0, end_ms: 100 });
		expect(b).toMatchObject({ label_id: 'label-2', start_ms: 100, end_ms: 250 });
		expect(state.segments).toEqual([a, b]);
	});

	it('segment_uids_are_unique_uuids', () => {
		const state = new AnnotationState();
		const uids = new Set<string>();
		for (let i = 0; i < 50; i++) {
			uids.add(state.addSegment('label-1', i * 10, i * 10 + 10).uid);
		}
		expect(uids.size).toBe(50);
		for (const uid of uids) {
			expect(uid).toMatch(
				/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
			);
		}
	});

	it('updateSegment_patches_boundaries', () => {
		const state = new AnnotationState();
		const seg = state.addSegment('label-1', 0, 100);
		state.updateSegment(seg.uid, { start_ms: 50 });
		expect(state.segments[0]).toMatchObject({ start_ms: 50, end_ms: 100 });
		state.updateSegment(seg.uid, { end_ms: 300 });
		expect(state.segments[0]).toMatchObject({ start_ms: 50, end_ms: 300 });
	});

	it('updateSegment_unknown_uid_is_safe_noop', () => {
		const state = new AnnotationState();
		state.addSegment('label-1', 0, 100);
		expect(() => state.updateSegment('no-such-uid', { start_ms: 5 })).not.toThrow();
		expect(state.segments[0]?.start_ms).toBe(0);
	});

	it('removeSegment_removes_only_that_segment', () => {
		const state = new AnnotationState();
		const a = state.addSegment('label-1', 0, 100);
		const b = state.addSegment('label-2', 100, 250);
		state.removeSegment(a.uid);
		expect(state.segments).toEqual([b]);
	});

	it('removeSegment_unknown_uid_is_safe_noop', () => {
		const state = new AnnotationState();
		state.addSegment('label-1', 0, 100);
		expect(() => state.removeSegment('no-such-uid')).not.toThrow();
		expect(state.segments).toHaveLength(1);
	});

	it('toPayload_maps_fields_to_camelcase_without_uid', () => {
		const state = new AnnotationState();
		state.addSegment('label-1', 0, 100);
		state.addSegment('label-2', 1500, 2500);
		expect(state.toPayload()).toEqual([
			{ labelId: 'label-1', startMs: 0, endMs: 100 },
			{ labelId: 'label-2', startMs: 1500, endMs: 2500 }
		]);
	});

	it('toPayload_empty_state_gives_empty_array', () => {
		expect(new AnnotationState().toPayload()).toEqual([]);
	});
});
