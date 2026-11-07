<?php

namespace Psr\Container {
    interface ContainerInterface
    {
        public function get(string $id): mixed;
    }
}

namespace Symfony\Component\DependencyInjection {
    interface ContainerInterface extends \Psr\Container\ContainerInterface
    {
        public function get(string $id, int $invalidBehavior = 1): ?object;
    }
}

namespace Symfony\Bundle\FrameworkBundle\Controller {
    abstract class AbstractController
    {
        protected function getParameter(string $name): array|bool|string|int|float|\UnitEnum|null { return null; }
    }
}

namespace Doctrine\ORM\Mapping {
    #[\Attribute]
    final class Entity
    {
        public function __construct(public ?string $repositoryClass = null) {}
    }
}

namespace Doctrine\ORM {
    /** @template T of object */
    class EntityRepository
    {
        /** @return T|null */
        public function find(mixed $id): ?object { return null; }
        public function __call(string $method, array $arguments): mixed { return null; }
    }

    interface EntityManagerInterface
    {
        /**
         * @template T of object
         * @param class-string<T> $className
         * @return EntityRepository<T>
         */
        public function getRepository(string $className): EntityRepository;
    }
}
