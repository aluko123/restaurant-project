-- Repair schema drift when 20260803 was applied from an older file revision.
ALTER TABLE inventory_count_entries
    ADD COLUMN IF NOT EXISTS storage_area_sort INT NOT NULL DEFAULT 0;

UPDATE inventory_count_entries e
SET storage_area_sort = COALESCE(a.sort_order, 0)
FROM inventory_items i
LEFT JOIN storage_areas a ON a.id = i.storage_area_id
WHERE e.inventory_item_id = i.id
  AND e.storage_area_sort = 0
  AND a.sort_order IS NOT NULL
  AND a.sort_order <> 0;
