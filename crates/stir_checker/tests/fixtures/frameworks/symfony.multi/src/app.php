<?php
// @framework symfony
declare(strict_types=1);

namespace App;

use Doctrine\ORM\EntityManagerInterface;
use Doctrine\ORM\EntityRepository;
use Doctrine\ORM\Mapping as ORM;
use Symfony\Bundle\FrameworkBundle\Controller\AbstractController;
use Symfony\Component\DependencyInjection\ContainerInterface;

class Mailer
{
    public function send(string $to): bool { return true; }
}

#[ORM\Entity(repositoryClass: ProductRepository::class)]
class Product
{
    public function name(): string { return ''; }
}

/** @extends EntityRepository<Product> */
class ProductRepository extends EntityRepository
{
    /** @return list<Product> */
    public function cheap(): array { return []; }
}

class Tag {}

function wants_int(int $n): void {}

class ShopController extends AbstractController
{
    public function run(EntityManagerInterface $em, ContainerInterface $container): void
    {
        $products = $em->getRepository(Product::class);
        $products->cheap();
        wants_int($products->findOneByName('lamp')); // error: argument.type
        wants_int($products->countByName('lamp'));
        $em->getRepository(Tag::class)->cheap(); // error: method.notFound
        $container->get('app.mailer')->send('a');
        $container->get(Mailer::class)->send('a');
        $container->get('app.mailer')->nope(); // error: method.notFound
        wants_int($this->getParameter('app.per_page'));
        wants_int($this->getParameter('kernel.debug')); // error: argument.type
    }
}
