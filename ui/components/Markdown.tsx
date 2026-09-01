import { memo } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";

interface MarkdownProps {
  children: string;
  className?: string;
}

const markdownPlugins = [remarkGfm];
const markdownComponents: Components = {
  a: ({ href, children }) => (
    <a href={href} target="_blank" rel="noreferrer">
      {children}
    </a>
  ),
};

export const Markdown = memo(function Markdown({ children, className = "" }: MarkdownProps) {
  return (
    <div className={`markdown ${className}`}>
      <ReactMarkdown
        remarkPlugins={markdownPlugins}
        components={markdownComponents}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
});
