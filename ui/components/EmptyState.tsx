import type { LucideIcon } from "lucide-react";

interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  detail: string;
}

export function EmptyState({ icon: Icon, title, detail }: EmptyStateProps) {
  return (
    <div className="empty-state">
      <span className="empty-icon">
        <Icon size={22} />
      </span>
      <h3>{title}</h3>
      <p>{detail}</p>
    </div>
  );
}
