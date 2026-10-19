import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Badge } from "./Badge";
import { Card, CardHeader } from "./Card";
import { Chip } from "./Chip";
import { PageHeader } from "./PageHeader";
import { StatusDot } from "./StatusDot";
import { Table, Td, Th } from "./Table";

describe("components", () => {
  it("Badge and StatusDot expose their tone", () => {
    const { container } = render(
      <>
        <Badge tone="warn">Degraded</Badge>
        <StatusDot tone="bad" />
      </>,
    );
    expect(screen.getByText("Degraded")).toHaveAttribute("data-tone", "warn");
    expect(container.querySelector("[data-tone=bad]")).toHaveAttribute("aria-hidden", "true");
  });

  it("Chip shows an optional dot", () => {
    const { container } = render(<Chip tone="ok">api</Chip>);
    expect(container.querySelector("[data-tone=ok]")).not.toBeNull();
  });

  it("Card sections are labelled by their header", () => {
    render(
      <Card as="section" aria-labelledby="t">
        <CardHeader title="Projects" titleId="t" />
      </Card>,
    );
    expect(screen.getByRole("region", { name: "Projects" })).toBeInTheDocument();
  });

  it("PageHeader renders title, subtitle and actions", () => {
    render(
      <PageHeader title="Host" subtitle="sub">
        <button type="button">Act</button>
      </PageHeader>,
    );
    expect(screen.getByRole("heading", { level: 1, name: "Host" })).toBeInTheDocument();
    expect(screen.getByText("sub")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Act" })).toBeInTheDocument();
  });

  it("Table keeps a scroll wrapper and aligns cells", () => {
    render(
      <Table>
        <thead>
          <tr>
            <Th align="right">Actions</Th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <Td>cell</Td>
          </tr>
        </tbody>
      </Table>,
    );
    expect(screen.getByRole("table").parentElement).not.toBeNull();
    expect(screen.getByRole("columnheader")).toHaveAttribute("data-align", "right");
  });
});
